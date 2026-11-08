use super::{
    lifecycle::{Snapshot, expected_matches},
    model::{Rule, domain, identifier},
};
use reqwest::{Client, Method, Url};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{future::Future, net::IpAddr};

pub(super) use crate::cloud_api::Failure;
use crate::cloud_api::transport;

pub(super) struct Cloudflare {
    client: Client,
    base: Url,
}

#[derive(Debug)]
pub(super) struct Outcome {
    pub record_id: String,
    pub status: &'static str,
}

#[derive(Clone, Deserialize)]
struct Record {
    id: String,
    name: String,
    #[serde(rename = "type")]
    record_type: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    ttl: u32,
    #[serde(default)]
    proxied: bool,
    #[serde(default)]
    comment: Option<String>,
}

impl Record {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            id: self.id.clone(),
            name: domain(&self.name).unwrap_or_else(|| self.name.clone()),
            kind: self.record_type.clone(),
            line: String::new(),
            values: vec![self.content.clone()],
            ttl: u64::from(self.ttl),
            proxied: self.proxied,
            active: true,
            marker: self.comment.clone(),
        }
    }
}

impl Cloudflare {
    pub fn new() -> Result<Self, Failure> {
        Ok(Self {
            client: crate::cloud_api::transport::client()?,
            base: Url::parse("https://api.cloudflare.com/client/v4/")
                .map_err(|_| Failure::from("client_error"))?,
        })
    }

    #[cfg(test)]
    pub fn local(base: &str) -> Self {
        let mut client = Self::new().unwrap();
        client.base = Url::parse(base).unwrap();
        assert_eq!(client.base.host_str(), Some("127.0.0.1"));
        client
    }

    pub(super) async fn call(
        &self,
        method: Method,
        path: &str,
        token: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<Value, Failure> {
        let url = self
            .base
            .join(path)
            .map_err(|_| Failure::from("invalid_response"))?;
        let mut request = self
            .client
            .request(method, url)
            .bearer_auth(token)
            .query(query);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| Failure::from("network_error"))?;
        let status = response.status();
        if status.as_u16() == 429 {
            return Err(transport::rate_limited(&response));
        }
        if !status.is_success() {
            return Err(match status.as_u16() {
                401 | 403 => "authentication_failed",
                404 => "resource_missing",
                400 | 409 => "provider_rejected",
                500..=599 => "provider_unavailable",
                300..=399 => "redirect_refused",
                _ => "http_error",
            }
            .into());
        }
        let envelope = transport::bounded_json(response).await?;
        if envelope["success"] != true
            || envelope
                .get("errors")
                .is_some_and(|errors| errors.as_array().is_none_or(|errors| !errors.is_empty()))
        {
            return Err("provider_rejected".into());
        }
        Ok(envelope)
    }

    async fn read_record(&self, rule: &Rule) -> Result<Option<Record>, Failure> {
        let config = &rule.config;
        if !identifier(&config.zone_id) {
            return Err("invalid_configuration".into());
        }
        let zone_path = format!("zones/{}", config.zone_id);
        let zone = self
            .call(Method::GET, &zone_path, &rule.api_token, &[], None)
            .await?;
        let zone_name = zone["result"]["name"]
            .as_str()
            .and_then(domain)
            .ok_or(Failure::from("invalid_response"))?;
        if zone["result"]["id"] != config.zone_id || zone_name.starts_with("*.") {
            return Err("invalid_response".into());
        }
        if config.record_name != zone_name
            && !config.record_name.ends_with(&format!(".{zone_name}"))
        {
            return Err("zone_mismatch".into());
        }
        if zone["result"]["status"] != "active" {
            return Err("zone_inactive".into());
        }
        let path = format!("{zone_path}/dns_records");
        let listing = self
            .call(
                Method::GET,
                &path,
                &rule.api_token,
                &[
                    ("name.exact", &config.record_name),
                    ("per_page", "100"),
                    ("page", "1"),
                ],
                None,
            )
            .await?;
        let records: Vec<Record> = serde_json::from_value(listing["result"].clone())
            .map_err(|_| Failure::from("invalid_response"))?;
        let total = listing["result_info"]["total_count"].as_u64();
        let pages = listing["result_info"]["total_pages"].as_u64();
        // A DDNS name has at most one record of each managed family. Refuse an
        // incomplete list rather than guessing which record may be changed.
        if total != Some(records.len() as u64)
            || pages.is_none_or(|pages| pages > 1)
            || records.len() > 100
        {
            return Err("record_conflict".into());
        }
        if records.iter().any(|r| {
            domain(&r.name).as_deref() != Some(config.record_name.as_str()) || !identifier(&r.id)
        }) {
            return Err("invalid_response".into());
        }
        if records
            .iter()
            .any(|r| matches!(r.record_type.as_str(), "CNAME" | "NS"))
        {
            return Err("record_conflict".into());
        }
        let matching: Vec<_> = records
            .iter()
            .filter(|r| r.record_type == config.record_type)
            .collect();
        if matching.len() > 1 {
            return Err("record_conflict".into());
        }
        let record = matching.first().copied();
        if record.is_some_and(|record| {
            rule.record_id.as_deref() != Some(record.id.as_str())
                && record.comment.as_deref() != Some(format!("apsp-ddns:{}", rule.id).as_str())
                && !config.adopt_existing
        }) {
            return Err("record_not_owned".into());
        }
        Ok(record.cloned())
    }

    pub(super) async fn inspect(&self, rule: &Rule) -> Result<Option<Snapshot>, Failure> {
        Ok(self.read_record(rule).await?.as_ref().map(Record::snapshot))
    }

    pub(super) async fn reconcile_expected_guarded<G, Check, Checked>(
        &self,
        rule: &Rule,
        ip: IpAddr,
        expected: Option<&Snapshot>,
        observed: &mut Option<Snapshot>,
        mut check: Check,
    ) -> Result<Outcome, Failure>
    where
        Check: FnMut() -> Checked,
        Checked: Future<Output = Result<G, Failure>>,
    {
        let config = &rule.config;
        let record = self.read_record(rule).await?;
        let current = record.as_ref().map(Record::snapshot);
        // The caller records what this read found, so it need not read twice.
        observed.clone_from(&current);
        expected_matches(current.as_ref(), expected)?;
        let matching: Vec<_> = record.iter().collect();
        let path = format!("zones/{}/dns_records", config.zone_id);
        let marker = format!("apsp-ddns:{}", rule.id);
        let ttl = if config.proxied { 1 } else { config.ttl };
        let (method, path, body, previous_id) = if let Some(record) = matching.first() {
            let owned = rule.record_id.as_deref() == Some(record.id.as_str())
                || record.comment.as_deref() == Some(marker.as_str());
            if !owned && !config.adopt_existing {
                return Err("record_not_owned".into());
            }
            if record.content.parse::<IpAddr>().ok() == Some(ip)
                && record.ttl == ttl
                && record.proxied == config.proxied
            {
                // A read that began while eligible cannot turn stale state into success.
                let _guard = check().await?;
                return Ok(Outcome {
                    record_id: record.id.clone(),
                    status: "unchanged",
                });
            }
            (
                Method::PATCH,
                format!("{path}/{}", record.id),
                json!({"name":config.record_name,"type":config.record_type,"content":ip.to_string(),"ttl":ttl,"proxied":config.proxied}),
                Some(record.id.as_str()),
            )
        } else {
            (
                Method::POST,
                path,
                json!({"name":config.record_name,"type":config.record_type,"content":ip.to_string(),"ttl":ttl,"proxied":config.proxied,"comment":marker}),
                None,
            )
        };
        // Keep the caller's local eligibility/lease guard through the bounded write.
        // External API side effects still cannot be rolled back by a database transaction.
        let _guard = check().await?;
        let response = self
            .call(method, &path, &rule.api_token, &[], Some(body))
            .await?;
        let written: Record = serde_json::from_value(response["result"].clone())
            .map_err(|_| Failure::from("invalid_response"))?;
        if !identifier(&written.id)
            || previous_id.is_some_and(|id| id != written.id)
            || domain(&written.name).as_deref() != Some(config.record_name.as_str())
            || written.record_type != config.record_type
            || written.content.parse::<IpAddr>().ok() != Some(ip)
            || written.ttl != ttl
            || written.proxied != config.proxied
            || (previous_id.is_none() && written.comment.as_deref() != Some(marker.as_str()))
        {
            return Err("invalid_response".into());
        }
        Ok(Outcome {
            record_id: written.id,
            status: "updated",
        })
    }
}

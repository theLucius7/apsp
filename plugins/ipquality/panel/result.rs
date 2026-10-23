use super::invalid;
use crate::{
    diagnostics::service::SectionContext,
    error::ApiResult,
    ip_quality::{IpQuality, QualityDatabase, QueryErrorKind},
};
use apsp_protocol::DiagnosticSectionUpdate;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

mod fields;
mod identity;
mod receipts;
pub(crate) use fields::validate_cached_fields;
pub(crate) use receipts::SOURCES;
use receipts::{Attempt, Status};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    schema: u32,
    plugin: String,
    version: String,
    job_id: Uuid,
    ip_version: String,
    artifact_sha256: String,
    source_commit: String,
    source_sha256: String,
    started_at: i64,
    #[serde(deserialize_with = "required_option")]
    finished_at: Option<i64>,
    #[serde(deserialize_with = "required_option")]
    egress_ip: Option<String>,
    #[serde(deserialize_with = "required_option")]
    upstream: Option<Value>,
    attempts: Vec<Attempt>,
}

pub(crate) struct Projection {
    pub ip_version: String,
    pub egress_ip: Option<String>,
    pub quality: Vec<IpQuality>,
}

pub(crate) fn parse(
    context: &SectionContext<'_>,
    update: &DiagnosticSectionUpdate,
) -> ApiResult<Projection> {
    let envelope: Envelope =
        serde_json::from_str(&update.text).map_err(|_| invalid("章节不是已登记格式的 JSON"))?;
    let canonical_ip = identity::validate(context, update, &envelope)?;
    let mut groups: BTreeMap<(&str, &str), Vec<&Attempt>> = BTreeMap::new();
    let mut requests = BTreeSet::new();
    for (index, attempt) in envelope.attempts.iter().enumerate() {
        if attempt.seq != index as u32 + 1 {
            return Err(invalid("请求收据序号必须连续且唯一"));
        }
        receipts::validate_attempt(attempt, &envelope, update.collected_at, context.expires_at)?;
        if let Some(url) = &attempt.url
            && !requests.insert((attempt.provider.as_str(), url.as_str()))
        {
            return Err(invalid("同一来源和地址不能重复请求"));
        }
        if attempt.provider != "egress-discovery" {
            groups
                .entry((&attempt.provider, &attempt.dataset))
                .or_default()
                .push(attempt);
        }
    }
    if canonical_ip.is_some()
        && !envelope.attempts.iter().any(|attempt| {
            attempt.provider == "egress-discovery" && attempt.status == Status::Succeeded
        })
    {
        return Err(invalid("出口地址没有本机发现收据"));
    }
    let Some(ip) = canonical_ip else {
        return Ok(Projection {
            ip_version: envelope.ip_version,
            egress_ip: None,
            quality: Vec::new(),
        });
    };
    let checked_at = envelope.finished_at.unwrap_or(update.collected_at);
    let mut providers: BTreeMap<String, Vec<QualityDatabase>> = BTreeMap::new();
    for ((provider, dataset), receipts) in groups {
        // An active snapshot cannot turn an absent future query into a failed attempt.
        if !update.complete
            && receipts
                .iter()
                .all(|attempt| attempt.status == Status::NotAttempted)
        {
            continue;
        }
        let confirmed = receipts
            .iter()
            .all(|attempt| attempt.status == Status::Succeeded);
        let fields = if confirmed {
            envelope
                .upstream
                .as_ref()
                .map(|raw| fields::parse_fields(raw, dataset))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let failed = receipts
            .iter()
            .rev()
            .find(|attempt| attempt.status != Status::Succeeded)
            .copied();
        let error_kind = failed
            .and_then(|attempt| attempt.error_kind)
            .or_else(|| fields.is_empty().then_some(QueryErrorKind::SchemaMismatch));
        let provider_id = format!("ipquality-node/{provider}");
        let latest = receipts
            .iter()
            .filter_map(|attempt| attempt.attempted_at.map(|at| (at, *attempt)))
            .max_by_key(|(at, _)| *at)
            .map(|(_, attempt)| attempt);
        let succeeded = confirmed && !fields.is_empty();
        let error = (!succeeded).then(|| {
            failed
                .and_then(|attempt| attempt.error_message.clone())
                .unwrap_or_else(|| "此源没有经本次请求收据确认的有效字段，信息未知".into())
        });
        providers
            .entry(provider_id.clone())
            .or_default()
            .push(QualityDatabase {
                database: format!("node-{dataset}"),
                label: format!("节点自查 · {dataset}"),
                status: if succeeded { "succeeded" } else { "failed" }.into(),
                fields,
                error,
                provider: provider_id,
                target_ip: Some(ip.clone()),
                attempted_at: latest.and_then(|attempt| attempt.attempted_at),
                elapsed_ms: latest.and_then(|attempt| attempt.elapsed_ms),
                error_kind,
                http_status: latest.and_then(|attempt| attempt.http_status),
                last_attempt_at: None,
                last_success_at: None,
                fresh_until: None,
                last_error: None,
                historical: false,
                available: None,
                unavailable_reason: None,
                execution: "node".into(),
                observed_ip: Some(ip.clone()),
                source: None,
            });
    }
    let quality = providers
        .into_iter()
        .map(|(provider, databases)| {
            let succeeded = databases
                .iter()
                .filter(|dataset| dataset.status == "succeeded")
                .count();
            IpQuality {
                ip: ip.clone(),
                checked_at,
                expires_at: 0,
                status: if succeeded == databases.len() {
                    "succeeded"
                } else if succeeded == 0 {
                    "failed"
                } else {
                    "partial"
                }
                .into(),
                last_attempt_at: databases
                    .iter()
                    .filter_map(|dataset| dataset.attempted_at)
                    .max(),
                provider,
                databases,
                last_success_at: None,
                fresh_until: None,
                last_error: BTreeMap::new(),
            }
        })
        .collect();
    Ok(Projection {
        ip_version: envelope.ip_version,
        egress_ip: Some(ip),
        quality,
    })
}

pub(super) fn required_option<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}

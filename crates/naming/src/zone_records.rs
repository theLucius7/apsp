//! Records in a Cloudflare zone that a later domain keeps under its own API token: service
//! access gives each personal address (ADR 0103) a proxied alias here, lists the aliases
//! back and deletes those no address uses. The caller brings the Cloudflare client with its
//! own settings and words Cloudflare's refusals; this module knows the record calls. Every
//! answer is judged by Cloudflare's envelope alone, so its codes reach the caller.

use crate::cloud_api::cloudflare::{Cloudflare, Error};
use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

/// A proxied CNAME record with an automatic TTL: `name` answers with Cloudflare's addresses
/// and is served from `target`.
pub struct Alias<'a> {
    pub name: &'a str,
    pub target: &'a str,
    pub comment: &'a str,
}

/// A record as a listing shows it.
#[derive(Deserialize)]
pub struct Listed {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Deserialize)]
struct Zone {
    name: String,
}

#[derive(Deserialize)]
struct RecordId {
    id: String,
}

/// The zone's name; `None` when Cloudflare answers without the zone.
pub async fn zone_name(api: &Cloudflare, token: &str, zone: &str) -> Result<Option<String>, Error> {
    let zone: Option<Zone> = api
        .result(Method::GET, &format!("zones/{zone}"), token, &[], None)
        .await?;
    Ok(zone.map(|zone| zone.name))
}

/// Creates `alias` in the zone; returns its record ID, or `None` when Cloudflare answers
/// without one.
pub async fn create_alias(
    api: &Cloudflare,
    token: &str,
    zone: &str,
    alias: &Alias<'_>,
) -> Result<Option<String>, Error> {
    let record = json!({"type":"CNAME","name":alias.name,"content":alias.target,"proxied":true,"ttl":1,"comment":alias.comment});
    let created: Option<RecordId> = api
        .result(
            Method::POST,
            &format!("zones/{zone}/dns_records"),
            token,
            &[],
            Some(record),
        )
        .await?;
    Ok(created.map(|record| record.id))
}

/// The zone's records of type `kind`, `per_page` at a time. A short page is the last one,
/// and at most `pages` pages are read, so that a huge zone cannot stall the caller.
pub async fn list(
    api: &Cloudflare,
    token: &str,
    zone: &str,
    kind: &str,
    per_page: usize,
    pages: u32,
) -> Result<Vec<Listed>, Error> {
    let path = format!("zones/{zone}/dns_records");
    let size = per_page.to_string();
    let mut records = Vec::new();
    for page in 1..=pages {
        let page = page.to_string();
        let batch: Vec<Listed> = api
            .result(
                Method::GET,
                &path,
                token,
                &[("type", kind), ("per_page", &size), ("page", &page)],
                None,
            )
            .await?
            .unwrap_or_default();
        let last = batch.len() < per_page;
        records.extend(batch);
        if last {
            break;
        }
    }
    Ok(records)
}

/// Deletes one record of the zone.
pub async fn delete(api: &Cloudflare, token: &str, zone: &str, id: &str) -> Result<(), Error> {
    api.result::<RecordId>(
        Method::DELETE,
        &format!("zones/{zone}/dns_records/{id}"),
        token,
        &[],
        None,
    )
    .await
    .map(drop)
}

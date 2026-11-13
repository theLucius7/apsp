#![forbid(unsafe_code)]
//! Names and certificates domain (ADR 0128): DNS accounts and records, DDNS rules that
//! keep records pointed at device addresses, DNS-01 validation records, and the
//! certificates the panel issues, renews and deploys. Certificate documents are stored
//! in the network configuration documents of basic networking and read from there.

mod acme;
mod acme_executor;
mod api;
mod certificate_deployments;
mod certificates;
mod cloudflare;
mod credentials;
pub mod dns01;
mod dns_accounts;
mod dns_challenges;
mod dns_record_actions;
mod dns_record_reconcile;
mod dns_record_rollback;
mod dns_record_spec;
mod dns_records;
mod dns_resolvers;
mod dns_wire;
mod history;
mod lifecycle;
pub mod locks;
mod migration;
mod model;
mod observation;
mod providers;
mod rollback;
mod settings;
#[cfg(test)]
mod tests;
mod worker;
pub mod zone_records;

pub use api::routes;
pub use worker::run;

pub(crate) use apsp_cloud_api as cloud_api;
pub(crate) use apsp_governance::{auth, control_center, releases};
pub(crate) use apsp_networking::network_configuration::{documents, models};
#[cfg(test)]
pub(crate) use apsp_panel_host::config;
pub(crate) use apsp_panel_host::{AppState, device_work, error, net, x509};
pub(crate) use apsp_resources::{device_channel, retirement, servers};

/// The certificate worker: issues and renews the certificates whose documents ask for it.
pub async fn run_certificates(state: AppState) {
    acme::run(state).await
}

use crate::error::{ApiError, ApiResult};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use model::Rule;

const MAX_RULES: i64 = 32;
const LEASE_SECS: i64 = 60;
const REQUEST_BUDGET: u64 = 20;

async fn load(pool: &PgPool, id: Uuid) -> ApiResult<Rule> {
    sqlx::query_as("SELECT * FROM ddns_rules WHERE id=$1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(ApiError::NotFound)
}

async fn editable(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<Rule> {
    let row: Rule = sqlx::query_as("SELECT * FROM ddns_rules WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    if row.lease_until > apsp_protocol::now_timestamp() {
        return Err(ApiError::Conflict("规则正在同步，请稍后操作".into()));
    }
    Ok(row)
}

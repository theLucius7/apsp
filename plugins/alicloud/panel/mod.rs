#![forbid(unsafe_code)]
//! Alibaba Cloud capabilities, organized by domain until they move into the domain crates
//! (ADR 0128 stage D): accounts and resources, CDT traffic and bills, power and address
//! automation, security groups and notifications.

// Each module belongs to one domain (tools/feature-boundaries.toml); the application
// composes their routes and workers (ADR 0128, ADR 0131).
mod addresses;
pub mod api;
mod billing;
mod client;
mod costs;
pub mod eip_rotation;
pub mod history;
mod inventory;
mod model;
mod notices;
pub mod operations;
pub mod overview;
pub mod power;
pub mod security_groups;
#[cfg(test)]
mod tests;
mod work;
pub mod worker;

pub(crate) use apsp_cloud_api as cloud_api;
#[cfg(test)]
pub(crate) use apsp_panel_host::config;
pub(crate) use apsp_panel_host::{AppState, auth, control_center, error, notifications, settings};

use crate::error::{ApiError, ApiResult};
use sqlx::{PgPool, Postgres, Transaction, types::Json};
use uuid::Uuid;

async fn lock(pool: &PgPool, account_id: Uuid) -> ApiResult<Transaction<'static, Postgres>> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,739104830))")
        .bind(account_id.to_string())
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

async fn account_on(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<model::Account> {
    sqlx::query_as("SELECT * FROM alicloud_accounts WHERE id=$1 AND NOT archived")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)
}

async fn resource(pool: &PgPool, id: Uuid) -> ApiResult<model::Resource> {
    sqlx::query_as("SELECT * FROM alicloud_resources WHERE id=$1 AND NOT archived")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Stores what the cloud reported for a resource.
async fn snapshot_on(
    tx: &mut Transaction<'_, Postgres>,
    resource: Uuid,
    snapshot: &model::Snapshot,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE alicloud_resources SET snapshot=$2,checked_at=$3,error_code=NULL WHERE id=$1",
    )
    .bind(resource)
    .bind(Json(snapshot))
    .bind(apsp_protocol::now_timestamp())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Whether an hourly retention pass is due, claiming it when it is.
fn prune_due(last: &std::sync::atomic::AtomicI64, now: i64) -> bool {
    use std::sync::atomic::Ordering;
    let previous = last.load(Ordering::Relaxed);
    now - previous >= 3600
        && last
            .compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
}

fn failure(error: crate::cloud_api::Failure) -> ApiError {
    ApiError::Conflict(format!("云接口未完成操作：{}", model::message(error.code)))
}

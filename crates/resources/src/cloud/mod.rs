//! Cloud accounts and resources (Alibaba Cloud): registration and credential references
//! (`api`, `client`), what the cloud reports about instances and addresses (`inventory`,
//! `addresses`, `model`), the question about work in progress that operations answers
//! (`work`) and the billing state costs keeps for each account (`billing`). Costs, power,
//! address rotation, security groups and notifications use these from their own crates;
//! costs keeps its columns in its own tables (ADR 0137 step 6), the resource rows still
//! carry the power columns of operations.

pub mod addresses;
pub mod api;
pub mod billing;
pub mod client;
pub mod inventory;
pub mod model;
pub mod work;

use crate::error::{ApiError, ApiResult};
use sqlx::{PgPool, Postgres, Transaction, types::Json};
use uuid::Uuid;

/// Opens a transaction that holds the account's lock: one writer per cloud account.
pub async fn lock(pool: &PgPool, account_id: Uuid) -> ApiResult<Transaction<'static, Postgres>> {
    let mut tx = pool.begin().await?;
    crate::locks::CLOUD_ACCOUNT
        .lock(&mut tx, &account_id.to_string())
        .await?;
    Ok(tx)
}

/// The account inside the caller's transaction, unless it is archived.
pub async fn account_on(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<model::Account> {
    sqlx::query_as("SELECT * FROM alicloud_accounts WHERE id=$1 AND NOT archived")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)
}

/// The resource, unless it is archived.
pub async fn resource(pool: &PgPool, id: Uuid) -> ApiResult<model::Resource> {
    sqlx::query_as("SELECT * FROM alicloud_resources WHERE id=$1 AND NOT archived")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Stores what the cloud reported for a resource.
pub async fn snapshot_on(
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
pub fn prune_due(last: &std::sync::atomic::AtomicI64, now: i64) -> bool {
    use std::sync::atomic::Ordering;
    let previous = last.load(Ordering::Relaxed);
    now - previous >= 3600
        && last
            .compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
}

/// The API error for a cloud call that did not complete.
pub fn failure(error: apsp_cloud_api::Failure) -> ApiError {
    ApiError::Conflict(format!("云接口未完成操作：{}", model::message(error.code)))
}

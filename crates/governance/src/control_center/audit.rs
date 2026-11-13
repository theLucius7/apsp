//! The management audit trail (`management_audit`): administrative requests as the request
//! guard sees them, sign-ins and sign-outs, credential resolutions and the changes other
//! owners record. Every row is written here.

use serde_json::Value;
use sqlx::PgExecutor;
use uuid::Uuid;

/// One row of the trail.
pub struct Entry<'a> {
    /// The administrator; none for a refused sign-in or a background credential resolution.
    pub admin_id: Option<i64>,
    /// The API token the request came with; none for a session.
    pub token_id: Option<Uuid>,
    pub action: &'a str,
    pub object_path: &'a str,
    pub request_diff: Value,
    pub result: Value,
    pub occurred_at: i64,
}

/// Writes `entry` and returns its id.
pub async fn record<'e>(
    executor: impl PgExecutor<'e>,
    entry: Entry<'_>,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("INSERT INTO management_audit(admin_id,token_id,action,object_path,request_diff,result,occurred_at) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id")
        .bind(entry.admin_id)
        .bind(entry.token_id)
        .bind(entry.action)
        .bind(entry.object_path)
        .bind(entry.request_diff)
        .bind(entry.result)
        .bind(entry.occurred_at)
        .fetch_one(executor)
        .await
}

/// Completes the row of an accepted request with its result and the state observed after it.
pub async fn complete<'e>(
    executor: impl PgExecutor<'e>,
    id: i64,
    result: Value,
    after: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE management_audit SET result=$2,request_diff=jsonb_set(request_diff,'{after}',$3) WHERE id=$1")
        .bind(id)
        .bind(result)
        .bind(after)
        .execute(executor)
        .await?;
    Ok(())
}

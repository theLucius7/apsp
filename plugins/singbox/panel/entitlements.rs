use crate::{
    AppState,
    auth::require_admin,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Serialize;
use sqlx::{FromRow, PgPool, Postgres, Transaction};

#[derive(Serialize, FromRow)]
pub struct Entitlement {
    pub user_id: i64,
    pub package_group_id: Option<i64>,
    pub package_name: Option<String>,
    pub monthly_bytes: Option<String>,
    pub reset_day: Option<i32>,
    pub reset_hour: Option<i32>,
    pub reset_minute: Option<i32>,
    pub timezone: Option<String>,
    pub starts_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub cycle_start: Option<i64>,
    pub next_reset: Option<i64>,
    pub used_bytes: String,
    pub status: String,
    pub allowed: bool,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Entitlement>> {
    require_admin(&state, &headers).await?;
    let value = sqlx::query_as("SELECT * FROM singbox_entitlements($1) WHERE user_id=$2")
        .bind(apsp_protocol::now_timestamp())
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(value))
}

// Serialize group topology and assignment changes before taking user/server locks.
pub(crate) async fn lock(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(73201001)")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Persist eligibility transitions so expiry/reset also publish without HTTP traffic.
/// Re-evaluating the same state does not keep pushing the debounce deadline forward.
pub async fn refresh(pool: &PgPool, at: i64) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    lock(&mut tx).await?;
    let changed: Vec<i64> = sqlx::query_scalar("SELECT e.user_id FROM singbox_entitlements($1) e LEFT JOIN singbox_entitlement_state s USING(user_id) WHERE s.allowed IS DISTINCT FROM e.allowed ORDER BY e.user_id")
        .bind(at).fetch_all(&mut *tx).await?;
    let mut servers: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT n.server_id FROM accesses a JOIN nodes n ON n.id=a.node_id WHERE a.user_id=ANY($1) ORDER BY n.server_id")
        .bind(&changed).fetch_all(&mut *tx).await?;
    let chain_servers: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT n.server_id FROM singbox_chains c JOIN nodes n ON n.id=c.entry_node_id LEFT JOIN singbox_chain_state cs ON cs.chain_id=c.id WHERE c.path_kind IN('legacy','mixed') AND cs.available IS DISTINCT FROM singbox_path_resources_available(c.id)")
        .fetch_all(&mut *tx).await?;
    servers.extend(chain_servers);
    let chain_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM singbox_chains WHERE path_kind='ordered' AND (deleted_at IS NULL OR phase<>'retired') ORDER BY id",
    )
    .fetch_all(&mut *tx)
    .await?;
    for id in chain_ids {
        let available = super::ordered_paths::chain_is_structurally_available(&mut tx, id).await?;
        let previous: Option<bool> =
            sqlx::query_scalar("SELECT available FROM singbox_chain_state WHERE chain_id=$1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if previous != Some(available) {
            servers.extend(super::ordered_paths::storage::referenced_servers(&mut tx, id).await?);
            sqlx::query("INSERT INTO singbox_chain_state(chain_id,available) VALUES($1,$2) ON CONFLICT(chain_id) DO UPDATE SET available=EXCLUDED.available").bind(id).bind(available).execute(&mut *tx).await?;
        }
    }
    servers.sort_unstable();
    servers.dedup();
    super::business::mark_dirty(&mut tx, &servers).await?;
    sqlx::query("INSERT INTO singbox_entitlement_state SELECT user_id,allowed FROM singbox_entitlements($1) WHERE user_id=ANY($2) ON CONFLICT(user_id) DO UPDATE SET allowed=EXCLUDED.allowed")
        .bind(at).bind(&changed).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO singbox_chain_state SELECT c.id,singbox_path_resources_available(c.id) FROM singbox_chains c WHERE c.path_kind IN('legacy','mixed') ON CONFLICT(chain_id) DO UPDATE SET available=EXCLUDED.available WHERE singbox_chain_state.available IS DISTINCT FROM EXCLUDED.available")
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

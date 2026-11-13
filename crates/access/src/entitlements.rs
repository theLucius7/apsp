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
use sqlx::{FromRow, PgPool};

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

/// Persist eligibility transitions so expiry/reset also publish without HTTP traffic.
/// Re-evaluating the same state does not keep pushing the debounce deadline forward.
///
/// Returns the users allowed at `at`. Evaluating every user's entitlement is the
/// most expensive statement of the publication cycle, so the cycle's later steps
/// (host membership, device-limit policies) use this evaluation.
pub async fn refresh(pool: &PgPool, at: i64) -> ApiResult<Vec<i64>> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    apsp_traffic::business::lock_publication(&mut tx).await?;
    let evaluated: Vec<(i64, bool, bool)> = sqlx::query_as("SELECT e.user_id,e.allowed,s.allowed IS DISTINCT FROM e.allowed FROM singbox_entitlements($1) e LEFT JOIN singbox_entitlement_state s USING(user_id) ORDER BY e.user_id")
        .bind(at).fetch_all(&mut *tx).await?;
    let allowed: Vec<i64> = evaluated
        .iter()
        .filter(|(_, allowed, _)| *allowed)
        .map(|(user, _, _)| *user)
        .collect();
    let (changed, changed_allowed): (Vec<i64>, Vec<bool>) = evaluated
        .iter()
        .filter(|(_, _, changed)| *changed)
        .map(|(user, allowed, _)| (*user, *allowed))
        .unzip();
    let mut servers: Vec<i64> = if changed.is_empty() {
        Vec::new()
    } else {
        sqlx::query_scalar("SELECT DISTINCT n.server_id FROM accesses a JOIN nodes n ON n.id=a.node_id WHERE a.user_id=ANY($1) ORDER BY n.server_id")
            .bind(&changed).fetch_all(&mut *tx).await?
    };
    // Chains that became available or unavailable change what their servers publish.
    servers.extend(apsp_traffic::ordered_paths::storage::refresh_availability(&mut tx).await?);
    servers.sort_unstable();
    servers.dedup();
    apsp_traffic::business::mark_dirty(&mut tx, &servers).await?;
    // Most ticks change nothing. The values come from the evaluation above (same
    // snapshot and instant), not from a second one.
    if !changed.is_empty() {
        sqlx::query("INSERT INTO singbox_entitlement_state(user_id,allowed) SELECT * FROM UNNEST($1::bigint[],$2::boolean[]) ON CONFLICT(user_id) DO UPDATE SET allowed=EXCLUDED.allowed")
            .bind(&changed).bind(&changed_allowed).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(allowed)
}

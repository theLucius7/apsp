//! The proxy users' part of the subscription board (ADR 0117): every user's
//! subscription in one read, with what a board needs at a glance and nothing
//! secret. Addresses and tokens never leave here; the user page reveals them after
//! a recent proof. Renewal items and sources come from their own endpoints.
use crate::{AppState, auth::require_admin, error::ApiResult};
use apsp_protocol::now_timestamp;
use axum::{Json, extract::State, http::HeaderMap};
use serde::Serialize;
use sqlx::FromRow;

/// The bridge reports "no next reset" as the last second of year 9999.
const NO_RESET: i64 = 253_402_300_799;

#[derive(Serialize)]
pub struct Board {
    now: i64,
    /// Whether fetches are recorded at all; without it "never fetched" means unknown.
    fetches_recorded: bool,
    /// Days a fetch record is kept, which bounds what "never fetched" can mean.
    fetch_retention_days: i32,
    subscriptions: Vec<Subscription>,
}

#[derive(Serialize, FromRow)]
pub struct Subscription {
    id: i64,
    name: String,
    status: String,
    allowed: bool,
    package_name: Option<String>,
    monthly_bytes: Option<String>,
    used_bytes: String,
    starts_at: Option<i64>,
    expires_at: Option<i64>,
    cycle_start: Option<i64>,
    next_reset: Option<i64>,
    groups: Vec<String>,
    /// Still rendered by the retained old panel; usage there is reported by the bridge.
    bridged: bool,
    last_fetch_at: Option<i64>,
    last_fetch_format: Option<String>,
    last_fetch_result: Option<String>,
    fetches_today: i64,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Board>> {
    require_admin(&state, &headers).await?;
    let now = now_timestamp();
    let mut tx = crate::read_snapshot(&state.pool).await?;
    sqlx::query("SET LOCAL statement_timeout='5s'")
        .execute(&mut *tx)
        .await?;
    // The same rule `subscriptions::record_fetch` applies before writing a record.
    let (fetches_recorded, fetch_retention_days): (bool, i32) =
        sqlx::query_as(super::subscriptions::FETCH_RECORDING)
            .fetch_one(&mut *tx)
            .await?;
    let subscriptions = sqlx::query_as::<_, Subscription>(
        "SELECT e.user_id AS id,u.name,e.status,e.allowed,e.package_name,e.monthly_bytes,e.used_bytes,\
         e.starts_at,e.expires_at,e.cycle_start,NULLIF(e.next_reset,$2) AS next_reset,\
         COALESCE((SELECT array_agg(g.name ORDER BY g.name) FROM singbox_user_policies p JOIN singbox_policy_groups g ON g.id=p.group_id WHERE p.user_id=e.user_id),'{}'::TEXT[]) AS groups,\
         EXISTS(SELECT 1 FROM singbox_nexus_identities i JOIN singbox_nexus_bridge b ON b.import_id=i.import_id WHERE b.enabled AND i.user_id=e.user_id) AS bridged,\
         f.created_at AS last_fetch_at,f.detail->>'format' AS last_fetch_format,f.detail->>'result' AS last_fetch_result,\
         (SELECT count(*) FROM singbox_operation_events v WHERE v.user_id=e.user_id AND v.action='subscription_fetch' AND v.created_at>$1-86400) AS fetches_today \
         FROM singbox_entitlements($1) e JOIN users u ON u.id=e.user_id \
         LEFT JOIN LATERAL (SELECT v.created_at,v.detail FROM singbox_operation_events v WHERE v.user_id=e.user_id AND v.action='subscription_fetch' ORDER BY v.created_at DESC LIMIT 1) f ON TRUE \
         ORDER BY e.user_id",
    )
    .bind(now)
    .bind(NO_RESET)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(Board {
        now,
        fetches_recorded,
        fetch_retention_days,
        subscriptions,
    }))
}

//! Owner-only HTTP interface. Node addresses and the imported inventory are shown
//! here, so the same boundary applies as for the Nexus inventory itself.

use super::{
    classify::ReachabilityState,
    store::{self, MAXIMUM_INTERVAL_MINUTES, MINIMUM_INTERVAL_MINUTES, Settings},
    targets::{self, Target},
    worker::CREDENTIAL_KIND,
};
use crate::{
    AppState, control_center,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

const MAX_TOKEN_BYTES: usize = 512;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/reachability", get(list))
        .route(
            "/reachability/settings",
            get(read_settings).put(update_settings),
        )
        .route("/reachability/run", post(run_now))
}

fn settings_view(settings: &Settings) -> Value {
    json!({
        "enabled":settings.enabled,
        "interval_minutes":settings.interval_minutes,
        "minimum_interval_minutes":MINIMUM_INTERVAL_MINUTES,
        "maximum_interval_minutes":MAXIMUM_INTERVAL_MINUTES,
        // Write-only: whether a token exists, never the token or its reference.
        "token_configured":settings.credential_id.is_some(),
        "revision":settings.revision,
        "next_run_at":settings.next_run_at,
        "run_requested":settings.run_requested_at.is_some(),
        "running":settings.lease_until.is_some(),
        "last_run":settings.last_run,
    })
}

fn target_view(target: &Target, row: Option<&store::TargetRow>) -> Value {
    let unmeasured = if target.tcp {
        ReachabilityState::Unknown
    } else {
        ReachabilityState::NotApplicable
    };
    let state = row.map_or(unmeasured, store::TargetRow::current_state);
    json!({
        "key":format!("{}:{}",target.host,target.port),
        "host":target.host,
        "port":target.port,
        "tcp":target.tcp,
        "state":state.as_str(),
        "blocked":state.is_blocked(),
        "pending_state":row.and_then(store::TargetRow::pending).map(ReachabilityState::as_str),
        "failing_streak":row.map_or(0,|row|row.failing_streak),
        "observed_state":row.and_then(|row|row.observed_state.as_deref()),
        "resolved_ip":row.and_then(|row|row.resolved_ip.as_deref()),
        "detail":row.map_or_else(||json!({}),|row|row.detail.clone()),
        "checked_at":row.and_then(|row|row.checked_at),
        "state_changed_at":row.and_then(|row|row.state_changed_at),
        "deferred_at":row.and_then(|row|row.deferred_at),
        "nodes":target.nodes,
        "risks":target.risks,
    })
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    let settings = store::read_settings(&state.pool).await?;
    let targets = targets::enumerate(&state.pool).await?;
    let rows = store::target_rows(&state.pool).await?;
    let views: Vec<Value> = targets
        .iter()
        .map(|target| target_view(target, rows.get(&(target.host.clone(), target.port))))
        .collect();
    Ok(Json(json!({
        "settings":settings_view(&settings),
        "targets":views,
        "events":store::recent_events(&state.pool).await?,
    })))
}

async fn read_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    Ok(Json(settings_view(
        &store::read_settings(&state.pool).await?,
    )))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateSettings {
    enabled: bool,
    interval_minutes: i32,
    revision: i64,
    #[serde(default)]
    api_token: Option<String>,
    #[serde(default)]
    clear_token: bool,
}

fn validate(input: &UpdateSettings) -> ApiResult<()> {
    if !(MINIMUM_INTERVAL_MINUTES..=MAXIMUM_INTERVAL_MINUTES).contains(&input.interval_minutes) {
        return Err(ApiError::BadRequest(format!(
            "检测间隔须在 {MINIMUM_INTERVAL_MINUTES} 至 {MAXIMUM_INTERVAL_MINUTES} 分钟之间"
        )));
    }
    if input.clear_token && input.api_token.is_some() {
        return Err(ApiError::BadRequest("不能同时填写新令牌和清除令牌".into()));
    }
    if let Some(token) = &input.api_token
        && (token.is_empty()
            || token.len() > MAX_TOKEN_BYTES
            || !token.bytes().all(|byte| byte.is_ascii_graphic()))
    {
        return Err(ApiError::BadRequest("API 令牌格式无效".into()));
    }
    Ok(())
}

async fn update_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<UpdateSettings>,
) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    validate(&input)?;
    let mut tx = state.pool.begin().await?;
    let current = store::lock_settings(&mut tx).await?;
    if current.revision != input.revision {
        return Err(ApiError::Conflict("设置已被修改，请刷新后重新确认".into()));
    }
    let mut credential = current.credential_id;
    if (input.clear_token || input.api_token.is_some())
        && let Some(previous) = credential.take()
    {
        sqlx::query("UPDATE credential_entries SET enabled=FALSE,updated_at=$2 WHERE id=$1")
            .bind(previous)
            .bind(now_timestamp())
            .execute(&mut *tx)
            .await?;
    }
    if let Some(token) = input.api_token {
        credential = Some(
            control_center::credentials::store_generated_on(
                &mut tx,
                "Globalping API 令牌",
                CREDENTIAL_KIND,
                json!({"token":token}),
            )
            .await?,
        );
    }
    sqlx::query(
        "UPDATE singbox_reachability_settings SET enabled=$1,interval_minutes=$2,credential_id=$3,revision=revision+1,run_requested_at=CASE WHEN $1 THEN run_requested_at END WHERE singleton",
    )
    .bind(input.enabled)
    .bind(input.interval_minutes)
    .bind(credential)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(settings_view(
        &store::read_settings(&state.pool).await?,
    )))
}

/// Only queues the run: the worker stays the single sender and applies the budget.
async fn run_now(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    let queued = sqlx::query(
        "UPDATE singbox_reachability_settings SET run_requested_at=$1 WHERE singleton AND enabled",
    )
    .bind(now_timestamp())
    .execute(&state.pool)
    .await?
    .rows_affected();
    if queued == 0 {
        return Err(ApiError::Conflict(
            "中国可达性检测尚未启用；启用后才会向第三方发送节点地址".into(),
        ));
    }
    Ok(Json(json!({"queued":true})))
}

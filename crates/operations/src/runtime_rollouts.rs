//! Staged rollouts of the proxy runtime's trusted version, as operations carries them out:
//! the first target alone as the canary, then fixed batches, each only after every earlier
//! target confirmed the version. Operations dispatches each target's read-only runtime
//! inspection and hands its result to the rollout; the rollout itself, whether a target is
//! ready and what a member confirmed belong to the traffic service
//! (`apsp_traffic::runtime_workflows::rollouts`).

use crate::{
    AppState, control_center,
    error::{ApiError, ApiResult},
};
use apsp_protocol::{RuntimeOperation, RuntimeOperationRequest, now_timestamp};
use apsp_traffic::{self as traffic, runtime_workflows::rollouts};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

/// The proxy runtime's module name in runtime operations.
const MODULE: &str = "singbox";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/traffic/runtime-rollouts", get(list).post(create))
        .route("/api/traffic/runtime-rollouts/{id}/advance", post(advance))
        .route("/api/traffic/runtime-rollouts/{id}/pause", post(pause))
        .route(
            "/api/traffic/runtime-rollouts/{id}/members/{server}/inspect",
            post(inspect_member),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewRollout {
    server_ids: Vec<i64>,
    canary_server_id: Option<i64>,
    runtime_version: String,
    batch_size: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pause {
    paused: bool,
}

/// A read-only inspection of the proxy runtime, valid for ten minutes.
fn inspection() -> RuntimeOperationRequest {
    RuntimeOperationRequest {
        id: Uuid::new_v4(),
        module: MODULE.into(),
        operation: RuntimeOperation::Inspect,
        expected_revision: None,
        session_filter: None,
        requested_at: now_timestamp(),
        expires_at: now_timestamp() + 600,
    }
}

/// Hands the results of finished inspections to their members: of one rollout, or of every
/// rollout.
async fn record_inspections(
    tx: &mut Transaction<'_, Postgres>,
    rollout: Option<Uuid>,
) -> ApiResult<()> {
    let pending = rollouts::pending_inspections_tx(tx, rollout).await?;
    if pending.is_empty() {
        return Ok(());
    }
    let requests: Vec<Uuid> = pending
        .iter()
        .map(|inspection| inspection.request_id)
        .collect();
    let results = crate::runtime_operations::results(tx, &requests).await?;
    for inspection in &pending {
        if let Some(result) = results.get(&inspection.request_id) {
            rollouts::record_inspection_tx(tx, inspection, result).await?;
        }
    }
    Ok(())
}

/// The rollouts' periodic step, run by the publication cycle: finished inspections reach
/// their members.
pub async fn record_inspection_results(state: &AppState) -> ApiResult<()> {
    let mut tx = state.pool.begin().await?;
    record_inspections(&mut tx, None).await?;
    tx.commit().await?;
    Ok(())
}

/// The rollout's members with what each device confirmed, once the results of their
/// finished inspections reached them.
async fn members(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<Vec<Value>> {
    record_inspections(tx, Some(id)).await?;
    rollouts::members_tx(tx, id).await
}

/// Dispatches the rollout's next batch: the canary alone, then `batch_size` targets at a
/// time, each batch only after every earlier target confirmed the version.
async fn dispatch(
    state: &AppState,
    headers: &HeaderMap,
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> ApiResult<()> {
    let rollout = rollouts::lock_tx(tx, id).await?;
    if rollout.paused {
        return Err(ApiError::Conflict("发布计划已暂停".into()));
    }
    let previous = members(tx, id).await?;
    if previous.iter().any(|v| v["confirmed"] != true) {
        return Err(ApiError::Conflict(
            "上一批尚未取得目标版本健康与实时运维确认，不能继续".into(),
        ));
    }
    let start = rollout.dispatched_count as usize;
    let count = if start == 0 {
        1
    } else {
        rollout.batch_size as usize
    };
    let end = (start + count).min(rollout.targets.len());
    if start == end {
        return Err(ApiError::Conflict("所有目标已分发，查看逐台结果".into()));
    }
    for server in &rollout.targets[start..end] {
        let artifact_sha256 =
            rollouts::require_target_ready_tx(state, headers, tx, &rollout, *server).await?;
        crate::fleet::ensure_accepts_tasks_tx(tx, *server).await?;
        if crate::runtime_operations::unresolved(tx, *server, MODULE).await? {
            return Err(ApiError::Conflict(format!(
                "服务器 #{server} 仍有运行时操作"
            )));
        }
        let inspect = inspection();
        crate::runtime_operations::enqueue(tx, *server, MODULE, &inspect).await?;
        rollouts::add_member_tx(
            tx,
            id,
            *server,
            inspect.id,
            inspect.requested_at,
            &artifact_sha256,
        )
        .await?;
    }
    rollouts::batch_dispatched_tx(tx, id, &rollout.targets[start..end], end).await
}

async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<NewRollout>,
) -> ApiResult<Json<Value>> {
    let administrator = control_center::require_capability(&state, &headers, "proxy:write").await?;
    control_center::require_recent_proof(&state, &headers).await?;
    let targets = rollouts::targets(&request.server_ids, request.canary_server_id)?;
    for server in &targets {
        control_center::require_server(&state, &headers, *server, "proxy:write").await?;
    }
    rollouts::require_supported(&state, &request.runtime_version, request.batch_size).await?;
    let mut tx = state.pool.begin().await?;
    traffic::business::lock_publication(&mut tx).await?;
    for server in &targets {
        crate::servers::lock(&mut tx, *server).await?;
        traffic::settings::require_enabled(&mut tx, *server).await?;
    }
    for previous in rollouts::unfinished_tx(&mut tx, &targets).await? {
        members(&mut tx, previous).await?;
    }
    if !rollouts::unfinished_tx(&mut tx, &targets).await?.is_empty() {
        return Err(ApiError::Conflict("所选服务器已有未完成发布计划".into()));
    }
    let id = rollouts::create_tx(
        &state,
        &mut tx,
        administrator,
        &request.runtime_version,
        &targets,
        request.batch_size,
    )
    .await?;
    dispatch(&state, &headers, &mut tx, id).await?;
    traffic::users::record_event(&mut tx,Some(administrator),None,"runtime_rollout_create",json!({"id":id,"targets":targets,"batch_size":request.batch_size,"canary_size":1,"runtime_version":traffic::RUNTIME_VERSION})).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":id,"state":"canary_dispatched","targets":targets}),
    ))
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Vec<Value>>> {
    control_center::require_capability(&state, &headers, "proxy:read").await?;
    let mut tx = state.pool.begin().await?;
    let rows = rollouts::recent_tx(&mut tx).await?;
    let mut result = Vec::new();
    for mut row in rows {
        let id: Uuid = serde_json::from_value(row["id"].clone()).map_err(anyhow::Error::from)?;
        row["members"] = json!(members(&mut tx, id).await?);
        result.push(row);
    }
    tx.commit().await?;
    Ok(Json(result))
}

async fn advance(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let administrator = control_center::require_capability(&state, &headers, "proxy:write").await?;
    control_center::require_recent_proof(&state, &headers).await?;
    for server in rollouts::targets_of(&state.pool, id).await? {
        control_center::require_server(&state, &headers, server, "proxy:write").await?;
    }
    let mut tx = state.pool.begin().await?;
    traffic::business::lock_publication(&mut tx).await?;
    dispatch(&state, &headers, &mut tx, id).await?;
    traffic::users::record_event(
        &mut tx,
        Some(administrator),
        None,
        "runtime_rollout_advance",
        json!({"id":id}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"state":"batch_dispatched"})))
}

async fn pause(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Pause>,
) -> ApiResult<Json<Value>> {
    let administrator = control_center::require_capability(&state, &headers, "proxy:write").await?;
    for server in rollouts::targets_of(&state.pool, id).await? {
        control_center::require_server(&state, &headers, server, "proxy:write").await?;
    }
    let mut tx = state.pool.begin().await?;
    rollouts::set_paused_tx(&mut tx, id, request.paused).await?;
    traffic::users::record_event(
        &mut tx,
        Some(administrator),
        None,
        "runtime_rollout_pause",
        json!({"id":id,"paused":request.paused}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"paused":request.paused,"already_dispatched_operations_cancelled":false}),
    ))
}

async fn inspect_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, server)): Path<(Uuid, i64)>,
) -> ApiResult<Json<Value>> {
    let administrator =
        control_center::require_server(&state, &headers, server, "proxy:write").await?;
    let mut tx = state.pool.begin().await?;
    crate::servers::lock(&mut tx, server).await?;
    crate::fleet::ensure_accepts_tasks_tx(&mut tx, server).await?;
    rollouts::lock_member_tx(&mut tx, id, server).await?;
    if crate::runtime_operations::unresolved(&mut tx, server, MODULE).await? {
        return Err(ApiError::Conflict(
            "设备仍有运维操作，等待原请求确认后再读取".into(),
        ));
    }
    let row = sqlx::query(&format!(
        "SELECT capabilities,last_seen,{} AS retiring FROM servers WHERE id=$1",
        crate::retirement::retiring_sql("$1")
    ))
    .bind(server)
    .fetch_one(&mut *tx)
    .await?;
    let caps: Value = row.get("capabilities");
    if row.get::<bool, _>("retiring")
        || !row
            .get::<Option<i64>, _>("last_seen")
            .is_some_and(|at| now_timestamp().saturating_sub(at) <= 60)
        || !caps.as_array().is_some_and(|v| {
            v.iter()
                .any(|c| c == apsp_protocol::RUNTIME_OPERATIONS_CAPABILITY)
        })
    {
        return Err(ApiError::Conflict(
            "设备离线、退役或不支持运行时读取".into(),
        ));
    }
    let inspect = inspection();
    crate::runtime_operations::enqueue(&mut tx, server, MODULE, &inspect).await?;
    rollouts::reinspect_member_tx(&mut tx, id, server, inspect.id, inspect.requested_at).await?;
    traffic::users::record_event(
        &mut tx,
        Some(administrator),
        None,
        "runtime_rollout_inspect",
        json!({"id":id,"server_id":server,"request_id":inspect.id,"mutation_replayed":false}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"state":"inspection_queued","request_id":inspect.id}),
    ))
}

#[cfg(test)]
mod tests;

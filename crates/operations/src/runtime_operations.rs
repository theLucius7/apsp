//! Runtime operations on a server's proxy runtime: the console's list and requests, what
//! each Agent fetches and reports, and the checks other operations work shares before
//! queuing one. Whether an operation fits the current deployment stays with the traffic
//! service (`apsp_traffic::deployments`).

use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
use apsp_protocol::{
    RUNTIME_OPERATIONS_CAPABILITY, RuntimeOperation, RuntimeOperationRequest,
    RuntimeOperationResult, TaskAck, now_timestamp,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgConnection, Row};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Create {
    pub operation: RuntimeOperation,
    pub expected_revision: Option<u64>,
}

#[derive(Serialize)]
pub struct OperationView {
    pub spec: RuntimeOperationRequest,
    pub result: Option<RuntimeOperationResult>,
    pub dispatched_at: Option<i64>,
}

#[derive(Serialize)]
pub struct View {
    pub supported: bool,
    pub online: bool,
    pub retiring: bool,
    pub operations: Vec<OperationView>,
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(server): Path<i64>,
) -> ApiResult<Json<View>> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    crate::servers::lock(&mut tx, server).await?;
    apsp_traffic::settings::require_enabled(&mut tx, server).await?;
    let row = sqlx::query(&format!(
        "SELECT capabilities,last_seen,{} AS retiring FROM servers WHERE id=$1",
        crate::retirement::retiring_sql("$1")
    ))
    .bind(server)
    .fetch_one(&mut *tx)
    .await?;
    let capabilities: Value = row.get("capabilities");
    let supported = capabilities
        .as_array()
        .is_some_and(|caps| caps.iter().any(|cap| cap == RUNTIME_OPERATIONS_CAPABILITY));
    let rows = sqlx::query("SELECT spec,result,dispatched_at FROM runtime_operations WHERE server_id=$1 AND module='singbox' ORDER BY requested_at DESC,id DESC LIMIT 20").bind(server).fetch_all(&mut *tx).await?;
    let operations = rows
        .into_iter()
        .map(|row| {
            Ok(OperationView {
                spec: serde_json::from_value(row.get("spec"))?,
                result: row
                    .get::<Option<Value>, _>("result")
                    .map(serde_json::from_value)
                    .transpose()?,
                dispatched_at: row.get("dispatched_at"),
            })
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()
        .map_err(anyhow::Error::from)?;
    tx.commit().await?;
    Ok(Json(View {
        supported,
        online: row
            .get::<Option<i64>, _>("last_seen")
            .is_some_and(|at| now_timestamp().saturating_sub(at) <= 60),
        retiring: row.get("retiring"),
        operations,
    }))
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(server): Path<i64>,
    Json(body): Json<Create>,
) -> ApiResult<Json<RuntimeOperationRequest>> {
    auth::require_admin(&state, &headers).await?;
    if matches!(
        body.operation,
        RuntimeOperation::InspectSessions | RuntimeOperation::CloseSessions
    ) {
        return Err(ApiError::BadRequest("请从代理用户的连接管理执行".into()));
    }
    let now = now_timestamp();
    let request = RuntimeOperationRequest {
        id: Uuid::new_v4(),
        module: "singbox".into(),
        operation: body.operation,
        expected_revision: body.expected_revision,
        session_filter: None,
        requested_at: now,
        expires_at: now + 600,
    };
    if !request.valid() {
        return Err(ApiError::BadRequest("运行时操作或期望版本无效".into()));
    }
    let mut tx = state.pool.begin().await?;
    crate::servers::lock(&mut tx, server).await?;
    apsp_traffic::settings::require_enabled(&mut tx, server).await?;
    crate::fleet::ensure_accepts_tasks_tx(&mut tx, server).await?;
    let row = sqlx::query(&format!(
        "SELECT capabilities,last_seen,{} AS pending,{} AS retiring FROM servers WHERE id=$1",
        apsp_traffic::publications::pending_sql("$1"),
        crate::retirement::retiring_sql("$1")
    ))
    .bind(server)
    .fetch_one(&mut *tx)
    .await?;
    let capabilities: Value = row.get("capabilities");
    if !capabilities
        .as_array()
        .is_some_and(|caps| caps.iter().any(|cap| cap == RUNTIME_OPERATIONS_CAPABILITY))
    {
        return Err(ApiError::Conflict(
            "Agent 尚不支持运行时运维，请先升级 Agent".into(),
        ));
    }
    if row.get::<bool, _>("retiring") {
        return Err(ApiError::Conflict(
            "服务器正在退役，不能执行运行时操作".into(),
        ));
    }
    if !row
        .get::<Option<i64>, _>("last_seen")
        .is_some_and(|at| now.saturating_sub(at) <= 60)
    {
        return Err(ApiError::Conflict(
            "设备离线，恢复连接后再执行运行时操作".into(),
        ));
    }
    if unresolved(&mut tx, server, "singbox").await? {
        return Err(ApiError::Conflict(
            "已有运行时操作等待确认，请等待设备回报".into(),
        ));
    }
    let latest: Option<i64> =
        sqlx::query_scalar("SELECT MAX(requested_at) FROM runtime_operations WHERE server_id=$1")
            .bind(server)
            .fetch_one(&mut *tx)
            .await?;
    if latest.is_some_and(|at| now.saturating_sub(at) < 10) {
        return Err(ApiError::Busy);
    }
    apsp_traffic::deployments::require_runtime_operation_fits(
        &mut tx,
        server,
        &request,
        row.get::<bool, _>("pending"),
    )
    .await?;
    enqueue(&mut tx, server, &request.module, &request).await?;
    sqlx::query("DELETE FROM runtime_operations WHERE server_id=$1 AND automation_job_id IS NULL AND result IS NOT NULL AND id NOT IN (SELECT id FROM runtime_operations WHERE server_id=$1 ORDER BY requested_at DESC,id DESC LIMIT 100)").bind(server).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(request))
}

/// SQL that holds while server `$1` takes no automation work at `$2`: it is closed, or a
/// fixed operation, a remote command or a diagnostic job there is unresolved.
fn unresolved_work_sql() -> String {
    format!(
        "{} OR EXISTS(SELECT 1 FROM fleet_operations WHERE server_id=$1 AND reconciled_at IS NULL AND status IN ('queued','dispatched','unknown')) OR EXISTS(SELECT 1 FROM remote_commands WHERE server_id=$1 AND state IN ('queued','claimed','running','cancel_requested')) OR EXISTS(SELECT 1 FROM diagnostic_jobs WHERE server_id=$1 AND (status IN ('queued','running','cleaning','cancel_requested') OR (NOT agent_completed AND job ? 'id')))",
        crate::fleet::closed_sql("$1", "$2")
    )
}

/// Whether an automation deployment may not be queued on the server at `now`: other work
/// is unresolved there, or any runtime operation still waits for its result.
pub(crate) async fn automation_blocked(
    connection: &mut PgConnection,
    server: i64,
    now: i64,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(&format!("SELECT {} OR EXISTS(SELECT 1 FROM runtime_operations WHERE server_id=$1 AND result IS NULL AND cancelled_at IS NULL AND reconciled_at IS NULL)", unresolved_work_sql()))
        .bind(server)
        .bind(now)
        .fetch_one(connection)
        .await?)
}

/// Whether an operation of the module on the server still waits for its result: queued or
/// dispatched, neither cancelled nor reconciled. A server runs one at a time per module.
pub(crate) async fn unresolved(
    connection: &mut PgConnection,
    server: i64,
    module: &str,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_operations WHERE server_id=$1 AND module=$2 AND result IS NULL AND cancelled_at IS NULL AND reconciled_at IS NULL)")
        .bind(server)
        .bind(module)
        .fetch_one(connection)
        .await?)
}

/// The results of those operations among `ids` that finished.
pub(crate) async fn results(
    connection: &mut PgConnection,
    ids: &[Uuid],
) -> ApiResult<HashMap<Uuid, Value>> {
    let rows: Vec<(Uuid, Value)> = sqlx::query_as(
        "SELECT id,result FROM runtime_operations WHERE id=ANY($1) AND result IS NOT NULL",
    )
    .bind(ids)
    .fetch_all(connection)
    .await?;
    Ok(rows.into_iter().collect())
}

pub async fn pending(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<RuntimeOperationRequest>>> {
    let server = crate::device_channel::require_agent(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    // Share the server lock order with retirement and operation creation.
    crate::servers::lock(&mut tx, server).await?;
    let accepting = match crate::fleet::ensure_accepts_tasks_tx(&mut tx, server).await {
        Ok(()) => true,
        Err(ApiError::Conflict(_)) => false,
        Err(error) => return Err(error),
    };
    let now = now_timestamp();
    let rows = sqlx::query("SELECT id,spec,dispatched_at,automation_job_id FROM runtime_operations WHERE server_id=$1 AND result IS NULL AND cancelled_at IS NULL AND reconciled_at IS NULL ORDER BY requested_at,id LIMIT 16 FOR UPDATE")
        .bind(server).fetch_all(&mut *tx).await?;
    let mut requests = Vec::new();
    for row in rows {
        let id: Uuid = row.get("id");
        let request: RuntimeOperationRequest =
            serde_json::from_value(row.get("spec")).map_err(anyhow::Error::from)?;
        if !request.valid() || request.id != id {
            return Err(ApiError::Conflict("保存的运行时操作身份无效".into()));
        }
        let permitted = if let Some(job) = row.get::<Option<Uuid>, _>("automation_job_id") {
            if request.expires_at <= now && row.get::<Option<i64>, _>("dispatched_at").is_none() {
                sqlx::query("UPDATE runtime_operations SET cancelled_at=$2 WHERE id=$1 AND dispatched_at IS NULL").bind(id).bind(now).execute(&mut *tx).await?;
                continue;
            }
            let actor: Option<i64> = sqlx::query_scalar("SELECT j.requested_by FROM operations_jobs j JOIN operations_server_locks l ON l.job_id=j.id AND l.server_id=$2 WHERE j.id=$1 AND j.status IN ('queued','running') AND j.cancel_requested_at IS NULL AND j.expires_at>$3 AND EXISTS(SELECT 1 FROM operations_target_steps t WHERE t.job_id=j.id AND t.server_id=$2 AND t.runtime_operation_id=$4 AND t.state IN ('queued','running'))")
                .bind(job).bind(server).bind(now).bind(id).fetch_optional(&mut *tx).await?;
            let authorized = if let Some(actor) = actor {
                crate::control_center::actor_server_allowed(
                    &state.pool,
                    actor,
                    server,
                    "operations:write",
                )
                .await?
                    && crate::control_center::actor_server_allowed(
                        &state.pool,
                        actor,
                        server,
                        "proxy:write",
                    )
                    .await?
            } else {
                false
            };
            let blocked: bool = sqlx::query_scalar(&format!("SELECT {}", unresolved_work_sql()))
                .bind(server)
                .bind(now)
                .fetch_one(&mut *tx)
                .await?;
            authorized
                && !blocked
                && crate::jobs::deployment_automation::automation_dispatch_matches_tx(
                    &mut tx, server, id,
                )
                .await?
        } else {
            accepting
        };
        if !permitted {
            continue;
        }
        sqlx::query("UPDATE runtime_operations SET dispatched_at=COALESCE(dispatched_at,$2) WHERE id=$1 AND cancelled_at IS NULL").bind(id).bind(now).execute(&mut *tx).await?;
        requests.push(request);
    }
    tx.commit().await?;
    Ok(Json(requests))
}

pub async fn complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(result): Json<RuntimeOperationResult>,
) -> ApiResult<Json<TaskAck>> {
    let server = crate::device_channel::require_agent(&state, &headers).await?;
    if result.id != id || !result.valid() || result.finished_at > now_timestamp() + 60 {
        return Err(ApiError::BadRequest("运行时操作结果无效".into()));
    }
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT spec,result,dispatched_at FROM runtime_operations WHERE id=$1 AND server_id=$2 FOR UPDATE").bind(id).bind(server).fetch_optional(&mut *tx).await?.ok_or(ApiError::NotFound)?;
    let request: RuntimeOperationRequest =
        serde_json::from_value(row.get("spec")).map_err(anyhow::Error::from)?;
    if request.module != result.module
        || request.operation != result.operation
        || result.finished_at < request.requested_at
        || row.get::<Option<i64>, _>("dispatched_at").is_none()
    {
        return Err(ApiError::Conflict("运行时操作身份或执行时间不匹配".into()));
    }
    if let Some(report) = &result.sessions {
        let filter = request
            .session_filter
            .as_ref()
            .ok_or_else(|| ApiError::Conflict("操作未授权连接结果".into()))?;
        if Some(report.revision) != request.expected_revision
            || report.observed_at > result.finished_at
            || report.observed_at < request.requested_at
            || report
                .connections
                .iter()
                .any(|c| !filter.principals.contains(&c.principal))
        {
            return Err(ApiError::Conflict("连接结果与请求身份不匹配".into()));
        }
    }
    let value = serde_json::to_value(&result).map_err(anyhow::Error::from)?;
    if let Some(previous) = row.get::<Option<Value>, _>("result") {
        if previous != value {
            return Err(ApiError::Conflict("已完成的运行时操作结果不可修改".into()));
        }
    } else {
        sqlx::query("UPDATE runtime_operations SET result=$2 WHERE id=$1")
            .bind(id)
            .bind(value)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(TaskAck { ids: vec![id] }))
}

/// Whether a server has an unexpired operation in flight or one requested in the last five
/// seconds; a new request waits for it instead of piling up.
pub async fn busy(connection: &mut sqlx::PgConnection, server: i64, now: i64) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_operations WHERE server_id=$1 AND result IS NULL AND cancelled_at IS NULL AND (spec->>'expires_at')::bigint>$2) OR EXISTS(SELECT 1 FROM runtime_operations WHERE server_id=$1 AND requested_at>$2-5)")
        .bind(server)
        .bind(now)
        .fetch_one(connection)
        .await?)
}

/// Queues one runtime operation for a module on a server.
pub async fn enqueue(
    connection: &mut sqlx::PgConnection,
    server: i64,
    module: &str,
    request: &RuntimeOperationRequest,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO runtime_operations(id,server_id,module,requested_at,spec) VALUES($1,$2,$3,$4,$5)")
        .bind(request.id)
        .bind(server)
        .bind(module)
        .bind(request.requested_at)
        .bind(serde_json::to_value(request).map_err(anyhow::Error::from)?)
        .execute(connection)
        .await?;
    Ok(())
}

/// The latest operation of a module on a server whose session filter names a principal
/// matching `pattern`, as `{id, requested_at, expires_at, operation, result}`.
pub async fn latest_for_principal(
    connection: &mut sqlx::PgConnection,
    server: i64,
    module: &str,
    pattern: &str,
) -> ApiResult<Vec<Value>> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('id',id,'requested_at',requested_at,'expires_at',(spec->>'expires_at')::bigint,'operation',spec->>'operation','result',result) FROM runtime_operations WHERE server_id=$1 AND module=$2 AND EXISTS(SELECT 1 FROM jsonb_array_elements_text(spec->'session_filter'->'principals') p WHERE p.value ~ $3) ORDER BY requested_at DESC,id DESC LIMIT 1")
        .bind(server)
        .bind(module)
        .bind(pattern)
        .fetch_all(connection)
        .await?)
}

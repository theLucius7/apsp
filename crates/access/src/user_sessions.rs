use crate::AppState;
use crate::error::ApiError;
use crate::error::ApiResult;
use apsp_protocol::{
    RuntimeOperation, RuntimeOperationRequest, RuntimeSessionFilter, now_timestamp,
};
use apsp_traffic::business;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    crate::control_center::require_capability(&state, &headers, "proxy:read").await?;
    let mut tx = apsp_traffic::read_snapshot(&state.pool).await?;
    let hosts = crate::user_limits::servers(&mut tx, id).await?;
    let mut result = Vec::new();
    for host in hosts {
        crate::control_center::require_server(&state, &headers, host, "proxy:read").await?;
        let row = sqlx::query("SELECT name,last_seen,capabilities FROM servers WHERE id=$1")
            .bind(host)
            .fetch_one(&mut *tx)
            .await?;
        let revision = apsp_traffic::deployments::applied_state(&mut tx, host)
            .await?
            .map(|applied| applied.applied_rev);
        let operations: Vec<Value> = crate::device_work::latest_runtime_for_principal_tx(
            &mut tx,
            host,
            "singbox",
            &format!("^u{id}_n[0-9]+$"),
        )
        .await?;
        let caps: Value = row.get("capabilities");
        result.push(json!({"server_id":host,"name":row.get::<String,_>("name"),"online":row.get::<Option<i64>,_>("last_seen").is_some_and(|at| now_timestamp().saturating_sub(at)<=60),"supported":caps.as_array().is_some_and(|v|v.iter().any(|v|v==apsp_protocol::RUNTIME_SESSIONS_CAPABILITY)),"revision":revision,"operation":operations.first()}));
    }
    tx.commit().await?;
    Ok(Json(json!({"servers":result})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    expected_revision: u64,
    close: bool,
    #[serde(default)]
    ids: Vec<String>,
}
pub async fn request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, host)): Path<(i64, i64)>,
    Json(body): Json<Request>,
) -> ApiResult<Json<Value>> {
    let actor = crate::control_center::require_server(
        &state,
        &headers,
        host,
        if body.close {
            "proxy:write"
        } else {
            "proxy:read"
        },
    )
    .await?;
    if body.close {
        crate::control_center::require_recent_proof(&state, &headers).await?;
    }
    let mut tx = state.pool.begin().await?;
    apsp_traffic::business::lock_publication(&mut tx).await?;
    business::lock_user(&mut tx, id).await?;
    business::lock_server(&mut tx, host).await?;
    crate::device_work::ensure_accepts_tasks_tx(&mut tx, host).await?;
    let row = sqlx::query("SELECT last_seen,capabilities,dirty_at FROM servers WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *tx)
        .await?;
    let (applied, bundle) = apsp_traffic::deployments::applied_state(&mut tx, host)
        .await?
        .and_then(|mut applied| applied.bundle.take().map(|bundle| (applied, bundle)))
        .ok_or_else(|| ApiError::Conflict("此服务器尚无已应用配置".into()))?;
    let caps: Value = row.get("capabilities");
    if !caps.as_array().is_some_and(|v| {
        v.iter()
            .any(|v| v == apsp_protocol::RUNTIME_SESSIONS_CAPABILITY)
    }) || !row
        .get::<Option<i64>, _>("last_seen")
        .is_some_and(|at| now_timestamp().saturating_sub(at) <= 60)
    {
        return Err(ApiError::Conflict("需要在线并支持连接管理的 Agent".into()));
    }
    if row.get::<Option<i64>, _>("dirty_at").is_some()
        || !applied.healthy
        || applied.applied_rev != applied.target_rev
        || Some(body.expected_revision) != u64::try_from(applied.applied_rev).ok()
    {
        return Err(ApiError::Conflict(
            "配置正在变化或状态不健康，请刷新后重试".into(),
        ));
    }
    let bundle: Value = serde_json::from_str(&bundle).map_err(anyhow::Error::from)?;
    let config: Value = serde_json::from_str(
        bundle["files"]["config.json"]
            .as_str()
            .ok_or_else(|| ApiError::Conflict("部署配置无效".into()))?,
    )
    .map_err(anyhow::Error::from)?;
    let prefix = format!("u{id}_n");
    let principals: Vec<String> = config
        .pointer("/experimental/v2ray_api/stats/users")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|name| {
            name.strip_prefix(&prefix)
                .is_some_and(|tail| !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(str::to_owned)
        .collect();
    let now = now_timestamp();
    let request = RuntimeOperationRequest {
        id: Uuid::new_v4(),
        module: "singbox".into(),
        operation: if body.close {
            RuntimeOperation::CloseSessions
        } else {
            RuntimeOperation::InspectSessions
        },
        expected_revision: Some(body.expected_revision),
        session_filter: Some(RuntimeSessionFilter {
            principals,
            ids: body.ids,
        }),
        requested_at: now,
        expires_at: now + 30,
    };
    if !request.valid() {
        return Err(ApiError::BadRequest(
            "用户在此版本中没有授权，或连接筛选无效".into(),
        ));
    }
    let busy = crate::device_work::runtime_busy_tx(&mut tx, host, now).await?;
    if busy {
        return Err(ApiError::Conflict("已有操作正在执行，请稍后刷新".into()));
    }
    crate::device_work::submit_runtime_tx(&mut tx, host, &request).await?;
    if body.close {
        crate::operations_workflows::event(
            &mut tx,
            Some(actor),
            Some(id),
            "connections_close_requested",
            json!({"server_id":host,"request_id":request.id,"revision":body.expected_revision}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(
        json!({"id":request.id,"expires_at":request.expires_at}),
    ))
}

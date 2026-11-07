use crate::{
    AppState, business,
    error::{ApiError, ApiResult},
};
use apsp_protocol::{
    RuntimeOperation, RuntimeOperationRequest, RuntimeSessionFilter, now_timestamp,
};
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
    let mut tx = crate::read_snapshot(&state.pool).await?;
    let hosts = crate::user_limits::servers(&mut tx, id).await?;
    let mut result = Vec::new();
    for host in hosts {
        crate::control_center::require_server(&state, &headers, host, "proxy:read").await?;
        let row=sqlx::query("SELECT s.name,s.last_seen,s.capabilities,m.applied_rev,m.target_rev FROM servers s LEFT JOIN server_module_status m ON m.server_id=s.id AND m.module='singbox' WHERE s.id=$1").bind(host).fetch_one(&mut *tx).await?;
        let operations:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'requested_at',requested_at,'expires_at',(spec->>'expires_at')::bigint,'operation',spec->>'operation','result',result) FROM runtime_operations WHERE server_id=$1 AND module='singbox' AND EXISTS(SELECT 1 FROM jsonb_array_elements_text(spec->'session_filter'->'principals') p WHERE p.value ~ $2) ORDER BY requested_at DESC,id DESC LIMIT 1")
            .bind(host).bind(format!("^u{id}_n[0-9]+$")).fetch_all(&mut *tx).await?;
        let caps: Value = row.get("capabilities");
        result.push(json!({"server_id":host,"name":row.get::<String,_>("name"),"online":row.get::<Option<i64>,_>("last_seen").is_some_and(|at| now_timestamp().saturating_sub(at)<=60),"supported":caps.as_array().is_some_and(|v|v.iter().any(|v|v==apsp_protocol::RUNTIME_SESSIONS_CAPABILITY)),"revision":row.get::<Option<i64>,_>("applied_rev"),"operation":operations.first()}));
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
    crate::entitlements::lock(&mut tx).await?;
    business::lock_user(&mut tx, id).await?;
    business::lock_server(&mut tx, host).await?;
    crate::fleet::ensure_accepts_tasks_tx(&mut tx, host).await?;
    let row=sqlx::query("SELECT s.last_seen,s.capabilities,s.dirty_at,m.applied_rev,m.target_rev,m.healthy,d.bundle FROM servers s JOIN server_module_status m ON m.server_id=s.id AND m.module='singbox' JOIN deployments d ON d.server_id=s.id AND d.module=m.module AND d.rev=m.applied_rev WHERE s.id=$1").bind(host).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::Conflict("此服务器尚无已应用配置".into()))?;
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
        || !row.get::<bool, _>("healthy")
        || row.get::<i64, _>("applied_rev") != row.get::<i64, _>("target_rev")
        || Some(body.expected_revision) != u64::try_from(row.get::<i64, _>("applied_rev")).ok()
    {
        return Err(ApiError::Conflict(
            "配置正在变化或状态不健康，请刷新后重试".into(),
        ));
    }
    let bundle: Value =
        serde_json::from_str(&row.get::<String, _>("bundle")).map_err(anyhow::Error::from)?;
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
    let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_operations WHERE server_id=$1 AND result IS NULL AND cancelled_at IS NULL AND (spec->>'expires_at')::bigint>$2) OR EXISTS(SELECT 1 FROM runtime_operations WHERE server_id=$1 AND requested_at>$2-5)").bind(host).bind(now).fetch_one(&mut *tx).await?;
    if busy {
        return Err(ApiError::Conflict("已有操作正在执行，请稍后刷新".into()));
    }
    sqlx::query("INSERT INTO runtime_operations(id,server_id,module,requested_at,spec) VALUES($1,$2,'singbox',$3,$4)").bind(request.id).bind(host).bind(now).bind(serde_json::to_value(&request).map_err(anyhow::Error::from)?).execute(&mut *tx).await?;
    if body.close {
        sqlx::query("INSERT INTO singbox_operation_events(administrator_id,user_id,action,detail,created_at) VALUES($1,$2,'connections_close_requested',$3,$4)").bind(actor).bind(id).bind(json!({"server_id":host,"request_id":request.id,"revision":body.expected_revision})).bind(now).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(
        json!({"id":request.id,"expires_at":request.expires_at}),
    ))
}

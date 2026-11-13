use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use apsp_protocol::{
    RuntimeValidationOperation, RuntimeValidationRequest, RuntimeValidationResult, TaskAck,
    now_timestamp,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

fn request(row: &sqlx::postgres::PgRow) -> ApiResult<RuntimeValidationRequest> {
    let operation = match row.get::<String, _>("operation").as_str() {
        "probe" => RuntimeValidationOperation::Probe,
        "barrier" => RuntimeValidationOperation::Barrier,
        _ => return Err(ApiError::BadRequest("运行时验证操作无效".into())),
    };
    Ok(RuntimeValidationRequest {
        id: row.get("id"),
        module: row.get("module"),
        scope: row.get("scope"),
        generation: row.get::<i64, _>("generation") as u64,
        operation,
        revision: row.get::<i64, _>("revision") as u64,
        config_hash: row.get("config_hash"),
        expires_at: row.get("expires_at"),
    })
}

pub async fn pending(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<RuntimeValidationRequest>>> {
    let server = crate::device_channel::require_agent(&state, &headers).await?;
    let rows = sqlx::query(&format!("SELECT id,module,scope,generation,operation,revision,config_hash,expires_at FROM runtime_validations WHERE server_id=$1 AND result IS NULL AND expires_at>$2 AND NOT {} ORDER BY requested_at,id LIMIT 16", crate::retirement::retiring_sql("$1"))).bind(server).bind(now_timestamp()).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.iter().map(request).collect::<ApiResult<Vec<_>>>()?,
    ))
}

pub async fn complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(result): Json<RuntimeValidationResult>,
) -> ApiResult<Json<TaskAck>> {
    let server = crate::device_channel::require_agent(&state, &headers).await?;
    if id != result.request.id
        || !result.valid()
        || result.checked_at > now_timestamp() + 60
        || (result.success && result.checked_at >= result.request.expires_at)
    {
        return Err(ApiError::BadRequest("运行时验证结果无效或已过期".into()));
    }
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT id,module,scope,generation,operation,revision,config_hash,expires_at,requested_at,result,result_digest FROM runtime_validations WHERE id=$1 AND server_id=$2 FOR UPDATE").bind(id).bind(server).fetch_optional(&mut *tx).await?.ok_or(ApiError::NotFound)?;
    if request(&row)? != result.request
        || result.checked_at < row.get::<i64, _>("requested_at").saturating_sub(60)
    {
        return Err(ApiError::Conflict("运行时验证身份或时间不匹配".into()));
    }
    let value = serde_json::to_value(&result).map_err(anyhow::Error::from)?;
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&result).map_err(anyhow::Error::from)?)
    );
    if let Some(previous) = row.get::<Option<Value>, _>("result") {
        if previous != value {
            return Err(ApiError::Conflict("运行时验证结果不可更改".into()));
        }
    } else {
        sqlx::query("UPDATE runtime_validations SET result=$2,result_digest=$3 WHERE id=$1")
            .bind(id)
            .bind(value)
            .bind(digest)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(TaskAck { ids: vec![id] }))
}

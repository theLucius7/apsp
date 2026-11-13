//! Reminders before a server's planned cancellation date, raised as incidents (costs).

use apsp_governance::control_center;
use apsp_panel_host::{
    AppState,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use axum::{Json, extract::State, http::HeaderMap};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<Value>>> {
    let actor = control_center::authenticate(&state, &headers).await?;
    if !actor.allows("operations:read") {
        return Err(ApiError::Forbidden("没有退订提醒读取授权".into()));
    }
    if !actor.allows("servers:read") {
        return Ok(Json(Vec::new()));
    }
    let permitted: Vec<i64> = actor
        .token_servers
        .as_ref()
        .unwrap_or(&actor.server_ids)
        .iter()
        .copied()
        .filter(|id| actor.allows_server(*id))
        .collect();
    let rows=sqlx::query("SELECT r.id AS record_id,r.server_id,s.name AS server_name,r.valid_until,r.reference,COALESCE(c.enabled,false) AS enabled,COALESCE(c.lead_secs,604800) AS lead_secs,c.notified_at,c.last_error FROM fleet_cost_records r JOIN servers s ON s.id=r.server_id LEFT JOIN operations_cancellation_reminders c ON c.record_id=r.id WHERE r.kind='cancellation' AND s.deleted_at IS NULL AND ($1 OR r.server_id=ANY($2)) ORDER BY r.occurred_at DESC,r.id DESC LIMIT 500").bind(actor.global_servers()).bind(permitted).fetch_all(&state.pool).await?;
    let mut values = Vec::new();
    for row in rows {
        let server: i64 = row.get("server_id");
        match control_center::require_server(&state, &headers, server, "operations:read").await {
            Ok(_) => {}
            Err(ApiError::Forbidden(_)) => continue,
            Err(error) => return Err(error),
        }
        match control_center::require_server(&state, &headers, server, "servers:read").await {
            Ok(_) => {}
            Err(ApiError::Forbidden(_)) => continue,
            Err(error) => return Err(error),
        }
        values.push(json!({"record_id":row.get::<Uuid,_>("record_id"),"server_id":server,"server_name":row.get::<String,_>("server_name"),"valid_until":row.get::<Option<i64>,_>("valid_until"),"reference":row.get::<String,_>("reference"),"enabled":row.get::<bool,_>("enabled"),"lead_secs":row.get::<i64,_>("lead_secs"),"notified_at":row.get::<Option<i64>,_>("notified_at"),"last_error":row.get::<Option<String>,_>("last_error")}));
    }
    Ok(Json(values))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configure {
    record_id: Uuid,
    enabled: bool,
    lead_secs: i64,
}
pub async fn configure(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Configure>,
) -> ApiResult<Json<Value>> {
    if control_center::authenticate(&state, &headers)
        .await?
        .token_id
        .is_some()
    {
        return Err(ApiError::Forbidden(
            "延迟退订提醒需由管理员会话配置，API 令牌不可创建或修改此计划".into(),
        ));
    }
    if !(3600..=2592000).contains(&input.lead_secs) {
        return Err(ApiError::BadRequest(
            "退订提醒提前量须为一小时至三十天".into(),
        ));
    }
    let row=sqlx::query("SELECT r.server_id,r.valid_until FROM fleet_cost_records r JOIN servers s ON s.id=r.server_id WHERE r.id=$1 AND r.kind='cancellation' AND s.deleted_at IS NULL").bind(input.record_id).fetch_optional(&state.pool).await?.ok_or(ApiError::NotFound)?;
    let server: i64 = row.get("server_id");
    let actor =
        control_center::require_server(&state, &headers, server, "operations:write").await?;
    control_center::require_server(&state, &headers, server, "servers:read").await?;
    if input.enabled && row.get::<Option<i64>, _>("valid_until").is_none() {
        return Err(ApiError::BadRequest(
            "此退订记录没有明确生效日期，不能启用日期提醒".into(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO operations_cancellation_reminders(record_id,enabled,lead_secs,configured_by,configured_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT(record_id) DO UPDATE SET enabled=$2,lead_secs=$3,configured_by=$4,configured_at=$5,last_error=NULL")
        .bind(input.record_id).bind(input.enabled).bind(input.lead_secs).bind(actor).bind(now_timestamp()).execute(&mut *tx).await?;
    if !input.enabled {
        sqlx::query("UPDATE operations_incident_deliveries SET status='cancelled' WHERE status='pending' AND incident_id=(SELECT incident_id FROM operations_cancellation_reminders WHERE record_id=$1)").bind(input.record_id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(
        json!({"record_id":input.record_id,"enabled":input.enabled,"side_effect":"仅提醒，不执行退订、停服或付款","replay_after_notified":false}),
    ))
}

pub async fn tick(state: &AppState) -> ApiResult<()> {
    let now = now_timestamp();
    let mut tx = state.pool.begin().await?;
    let rows=sqlx::query("SELECT c.record_id,c.configured_by,c.lead_secs,r.server_id,r.valid_until,r.reference,s.name FROM operations_cancellation_reminders c JOIN fleet_cost_records r ON r.id=c.record_id JOIN servers s ON s.id=r.server_id WHERE c.enabled AND c.notified_at IS NULL AND r.kind='cancellation' AND r.valid_until IS NOT NULL AND r.valid_until-c.lead_secs<=$1 AND s.deleted_at IS NULL ORDER BY r.valid_until LIMIT 32 FOR UPDATE OF c SKIP LOCKED").bind(now).fetch_all(&mut *tx).await?;
    for row in rows {
        let record: Uuid = row.get("record_id");
        let server: i64 = row.get("server_id");
        let actor: i64 = row.get("configured_by");
        if !control_center::actor_server_allowed(&state.pool, actor, server, "operations:write")
            .await?
            || !control_center::actor_server_allowed(&state.pool, actor, server, "servers:read")
                .await?
        {
            sqlx::query("UPDATE operations_cancellation_reminders SET enabled=false,last_error='配置者服务器或提醒权限已撤销，提醒已暂停' WHERE record_id=$1").bind(record).execute(&mut *tx).await?;
            continue;
        }
        let source = format!("cancellation:{record}");
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM operations_incidents WHERE source_key=$1 ORDER BY opened_at LIMIT 1",
        )
        .bind(&source)
        .fetch_optional(&mut *tx)
        .await?;
        let incident = if let Some(id) = existing {
            id
        } else {
            apsp_panel_host::evidence::open_incident_tx(&mut tx, apsp_panel_host::evidence::IncidentReport { key: &source, server: Some(server), title: &format!("服务器 {} 的计划退订日期需要核对",row.get::<String,_>("name")), severity: "info", evidence: json!({"record_id":record,"valid_until":row.get::<i64,_>("valid_until"),"reference":row.get::<String,_>("reference"),"lead_secs":row.get::<i64,_>("lead_secs"),"source":"人工费用台账：计划退订生效日期","action_executed":false,"observed_at":now}), observed_at: now }).await?;
            sqlx::query_scalar(
                "SELECT id FROM operations_incidents WHERE source_key=$1 AND status<>'resolved'",
            )
            .bind(&source)
            .fetch_one(&mut *tx)
            .await?
        };
        sqlx::query("UPDATE operations_cancellation_reminders SET notified_at=$2,incident_id=$3,last_error=NULL WHERE record_id=$1").bind(record).bind(now).bind(incident).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

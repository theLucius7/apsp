//! Enrollment templates and enrolling many servers from one template (resources).

use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
    server_assets::AssetSettings,
};
use apsp_protocol::{AgentSettings, ProbeSpec, now_timestamp, telemetry::TelemetrySettings};
use axum::{Json, extract::State, http::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

/// What servers enrolled from a template start with; the procurement record is part of the
/// asset settings.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TemplateConfig {
    pub agent_settings: AgentSettings,
    pub telemetry_settings: TelemetrySettings,
    pub asset_settings: AssetSettings,
    pub probes: Vec<ProbeSpec>,
    pub alerts: Vec<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateInput {
    id: Option<Uuid>,
    name: String,
    config: TemplateConfig,
}

fn normalize(mut config: TemplateConfig) -> ApiResult<TemplateConfig> {
    config.asset_settings = config.asset_settings.normalized()?;
    if !config.agent_settings.valid()
        || !config.telemetry_settings.valid()
        || config.probes.len() > 32
        || config.alerts.len() > 32
    {
        return Err(ApiError::BadRequest(
            "模板采集参数、拨测或告警数量无效".into(),
        ));
    }
    for spec in &mut config.probes {
        spec.normalize();
        crate::server_observation::prepare_probe(spec)?;
    }
    for alert in &config.alerts {
        crate::server_observation::check_alert_rule(alert)?;
    }
    Ok(config)
}

pub async fn templates(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<Value>>> {
    crate::control_center::require_capability(&state, &headers, "servers:read").await?;
    let rows =
        sqlx::query("SELECT id,name,config,updated_at FROM fleet_templates ORDER BY name,id")
            .fetch_all(&state.pool)
            .await?;
    Ok(Json(rows.into_iter().map(|r| json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"config":r.get::<Value,_>("config"),"updated_at":r.get::<i64,_>("updated_at")})).collect()))
}

pub async fn save_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<TemplateInput>,
) -> ApiResult<Json<Value>> {
    crate::control_center::require_capability(&state, &headers, "servers:write").await?;
    let name = crate::text::bounded(&input.name, 128)?;
    if name.is_empty() {
        return Err(ApiError::BadRequest("模板名称不能为空".into()));
    }
    let config = normalize(input.config)?;
    validate_mirror(&config, &state)?;
    let id = input.id.unwrap_or_else(Uuid::new_v4);
    sqlx::query("INSERT INTO fleet_templates(id,name,config,created_at,updated_at) VALUES($1,$2,$3,$4,$4) ON CONFLICT(id) DO UPDATE SET name=$2,config=$3,updated_at=$4")
        .bind(id).bind(&name).bind(json!(config)).bind(now_timestamp()).execute(&state.pool).await?;
    Ok(Json(json!({"id":id,"name":name,"config":config})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchInput {
    template_id: Option<Uuid>,
    names: Vec<String>,
}

pub async fn batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<BatchInput>,
) -> ApiResult<Json<Vec<Value>>> {
    crate::control_center::require_capability(&state, &headers, "servers:write").await?;
    if input.names.is_empty() || input.names.len() > 100 {
        return Err(ApiError::BadRequest("一次登记需要 1–100 台服务器".into()));
    }
    let config = if let Some(id) = input.template_id {
        let value: Value = sqlx::query_scalar("SELECT config FROM fleet_templates WHERE id=$1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(ApiError::NotFound)?;
        normalize(serde_json::from_value(value).map_err(anyhow::Error::from)?)?
    } else {
        TemplateConfig::default()
    };
    validate_mirror(&config, &state)?;
    let mut results = Vec::new();
    for (index, name) in input.names.into_iter().enumerate() {
        let result = register(&state, &config, &name).await;
        results.push(match result {
            Ok(value) => {
                json!({"index":index,"name":name,"status":"registered","registration":value})
            }
            Err(error) => {
                json!({"index":index,"name":name,"status":"failed","error":error.to_string()})
            }
        });
    }
    Ok(Json(results))
}

async fn register(state: &AppState, config: &TemplateConfig, name: &str) -> ApiResult<Value> {
    let name = crate::text::bounded(name, 128)?;
    if name.is_empty() {
        return Err(ApiError::BadRequest("服务器名称不能为空".into()));
    }
    let token = auth::random_token();
    let now = now_timestamp();
    let mut tx = state.pool.begin().await?;
    crate::server_observation::lock_latency_tasks_tx(&mut tx).await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO servers(name,agent_settings,asset_settings) VALUES($1,$2,$3) RETURNING id",
    )
    .bind(name)
    .bind(json!(config.agent_settings))
    .bind(json!(config.asset_settings))
    .fetch_one(&mut *tx)
    .await?;
    crate::server_observation::save_telemetry_settings_tx(&mut tx, id, &config.telemetry_settings)
        .await?;
    for spec in &config.probes {
        let mut spec = spec.clone();
        spec.id = Uuid::new_v4();
        sqlx::query("INSERT INTO network_probes(id,server_id,spec) VALUES($1,$2,$3)")
            .bind(spec.id)
            .bind(id)
            .bind(json!(spec))
            .execute(&mut *tx)
            .await?;
    }
    for alert in &config.alerts {
        let mut spec = alert.clone();
        spec["all_servers"] = json!(false);
        spec["server_ids"] = json!([id]);
        sqlx::query("INSERT INTO alert_rules(id,spec,revision) VALUES($1,$2,1)")
            .bind(Uuid::new_v4())
            .bind(spec)
            .execute(&mut *tx)
            .await?;
    }
    crate::server_observation::assign_default_latency_tasks_tx(&mut tx, id).await?;
    sqlx::query("INSERT INTO enrollment_tokens(token_hash,server_id,expires_at) VALUES($1,$2,$3)")
        .bind(auth::hash_token(&token))
        .bind(id)
        .bind(now + 86400)
        .execute(&mut *tx)
        .await?;
    crate::evidence::record_event_tx(&mut tx, id, "registered", json!({"template":true})).await?;
    tx.commit().await?;
    Ok(json!({"server_id":id,"token":token,"expires_at":now+86400,"enrolled":false}))
}

fn validate_mirror(config: &TemplateConfig, state: &AppState) -> ApiResult<()> {
    if !config.asset_settings.agent_mirror.is_empty()
        && let (Ok(mirror), Ok(panel)) = (
            reqwest::Url::parse(&config.asset_settings.agent_mirror),
            reqwest::Url::parse(&state.config.public_url),
        )
        && mirror.origin() == panel.origin()
    {
        return Err(ApiError::BadRequest(
            "Agent 下载镜像必须独立于面板，保留 GitHub 来源与验签".into(),
        ));
    }
    Ok(())
}

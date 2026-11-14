//! A server's operations profile: its procurement record (kept in the asset settings), local
//! access policy, lifecycle and migration to another server (resources). The costs recorded
//! for the server are costs' ledger (`apsp_costs`).

use crate::{
    AppState,
    error::{ApiError, ApiResult},
    server_assets::{AssetSettings, Procurement},
};
use apsp_protocol::{fleet::AccessPolicy, now_timestamp};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    crate::control_center::require_server(&state, &headers, id, "servers:read").await?;
    let row=sqlx::query("SELECT s.static_info,s.capabilities,s.agent_settings,s.asset_settings,p.policy,p.lifecycle,p.maintenance_from,p.maintenance_until,p.maintenance_reason FROM servers s LEFT JOIN fleet_profiles p ON p.server_id=s.id WHERE s.id=$1 AND s.deleted_at IS NULL")
        .bind(id).fetch_optional(&state.pool).await?.ok_or(ApiError::NotFound)?;
    // The profile shows the process and pressure detail that polled server reads leave out.
    let latest =
        crate::server_observation::stored_metrics(&mut *state.pool.acquire().await?, &[id])
            .await?
            .remove(&id)
            .unwrap_or_default();
    let static_info: Value = row.get("static_info");
    let settings: AssetSettings =
        serde_json::from_value(row.get("asset_settings")).map_err(anyhow::Error::from)?;
    let asset = procurement_view(settings.procurement.as_ref())?;
    let purchased = asset.get("purchased").cloned().unwrap_or_else(|| json!({}));
    let policy: AccessPolicy = serde_json::from_value(
        row.get::<Option<Value>, _>("policy")
            .unwrap_or_else(|| json!({})),
    )
    .map_err(anyhow::Error::from)?;
    let digest = profile_digest(&asset, &policy)?;
    let observed = json!({"cpu_cores":static_info["cpu_cores"],"cpu_model":static_info["cpu_model"],"memory_bytes":static_info["memory_total"],"disk_bytes":static_info["disk_total"],"bandwidth_mbps":null});
    let related = references(&state.pool, id).await?;
    Ok(Json(
        json!({"server_id":id,"asset":asset,"policy":policy,"digest":digest,"lifecycle":row.get::<Option<String>,_>("lifecycle").unwrap_or_else(||"active".into()),"maintenance_from":row.get::<Option<i64>,_>("maintenance_from"),"maintenance_until":row.get::<Option<i64>,_>("maintenance_until"),"maintenance_reason":row.get::<Option<String>,_>("maintenance_reason"),"capabilities":row.get::<Value,_>("capabilities"),"purchased":purchased,"observed":observed,"asset_settings":row.get::<Value,_>("asset_settings"),"related":related,"sampled_at":latest.sampled_at,"processes":latest.metrics["process_resources"],"pressure":latest.metrics["system_pressure"]}),
    ))
}

/// A server's local access policy, lifecycle and maintenance window.
pub struct Profile {
    pub policy: Value,
    pub lifecycle: String,
    pub maintenance_from: Option<i64>,
    pub maintenance_until: Option<i64>,
}

/// The server's profile under a share lock in the caller's transaction, so it cannot
/// change before a plan that records it is stored; `None` while the server has none.
pub async fn locked_profile(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    server_id: i64,
) -> Result<Option<Profile>, sqlx::Error> {
    let row=sqlx::query("SELECT policy,lifecycle,maintenance_from,maintenance_until FROM fleet_profiles WHERE server_id=$1 FOR SHARE")
        .bind(server_id).fetch_optional(&mut **tx).await?;
    Ok(row.map(|row| Profile {
        policy: row.get("policy"),
        lifecycle: row.get("lifecycle"),
        maintenance_from: row.get("maintenance_from"),
        maintenance_until: row.get("maintenance_until"),
    }))
}

/// SQL that holds while the server's own lifecycle blocks new work at `at`: a retirement is
/// recorded, the server is draining or retired, or its maintenance lifecycle covers `at`.
/// `server` is a parameter or a qualified column and `at` a time in seconds, both SQL. Every
/// check before work goes to a server starts from this; operations adds the maintenance
/// windows that block new tasks, which it owns.
pub fn blocks_work_sql(server: &str, at: &str) -> String {
    format!(
        "({} OR EXISTS(SELECT 1 FROM fleet_profiles blocking WHERE blocking.server_id={server} \
         AND (blocking.lifecycle IN ('draining','retired') OR {})))",
        crate::retirement::retiring_sql(server),
        maintenance_window("blocking", at)
    )
}

/// SQL that holds while the server is in its own maintenance lifecycle at `at` (both SQL, as
/// for [`blocks_work_sql`]); notifications about it are held back meanwhile.
pub fn maintenance_sql(server: &str, at: &str) -> String {
    format!(
        "EXISTS(SELECT 1 FROM fleet_profiles maintained WHERE maintained.server_id={server} AND {})",
        maintenance_window("maintained", at)
    )
}

/// The profile row `profile` is in its maintenance lifecycle at `at`; an open start or end
/// is unbounded.
fn maintenance_window(profile: &str, at: &str) -> String {
    format!(
        "({profile}.lifecycle='maintenance' AND COALESCE({profile}.maintenance_from,0)<={at} \
         AND ({profile}.maintenance_until IS NULL OR {profile}.maintenance_until>{at}))"
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
    asset: Procurement,
    policy: AccessPolicy,
    expected_digest: String,
}

/// The procurement record as the profile shows it: `{}` when none is recorded.
fn procurement_view(procurement: Option<&Procurement>) -> ApiResult<Value> {
    Ok(match procurement {
        Some(procurement) => serde_json::to_value(procurement).map_err(anyhow::Error::from)?,
        None => json!({}),
    })
}
fn profile_digest(asset: &Value, policy: &AccessPolicy) -> ApiResult<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!({"asset":asset,"policy":policy}))
                .map_err(anyhow::Error::from)?
        )
    ))
}
pub async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<ProfileInput>,
) -> ApiResult<Json<Value>> {
    crate::control_center::require_server(&state, &headers, id, "servers:write").await?;
    crate::control_center::require_recent_proof(&state, &headers).await?;
    let procurement = input.asset.normalized()?;
    crate::control_center::require_capability(&state, &headers, "security:write").await?;
    let policy = normalize_policy(input.policy)?;
    let mut tx = state.pool.begin().await?;
    let settings: AssetSettings = serde_json::from_value(
        crate::servers::lock_row(
            &mut tx,
            id,
            crate::servers::RowLock::Update,
            "asset_settings",
        )
        .await?
        .ok_or(ApiError::NotFound)?
        .get("asset_settings"),
    )
    .map_err(anyhow::Error::from)?;
    let original_policy: AccessPolicy =
        sqlx::query_scalar::<_, Value>("SELECT policy FROM fleet_profiles WHERE server_id=$1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .map(serde_json::from_value)
            .transpose()
            .map_err(anyhow::Error::from)?
            .unwrap_or_default();
    if input.expected_digest
        != profile_digest(
            &procurement_view(settings.procurement.as_ref())?,
            &original_policy,
        )?
    {
        return Err(ApiError::Conflict(
            "资产或授权已由其他窗口修改。当前表单保留，请保存草稿、读取最新资料后再确认差异".into(),
        ));
    }
    let asset = procurement_view(procurement.as_ref())?;
    let digest = profile_digest(&asset, &policy)?;
    crate::server_assets::set_procurement_tx(&mut tx, id, procurement.as_ref()).await?;
    sqlx::query("INSERT INTO fleet_profiles(server_id,policy,updated_at) VALUES($1,$2,$3) ON CONFLICT(server_id) DO UPDATE SET policy=$2,updated_at=$3")
        .bind(id).bind(json!(policy)).bind(now_timestamp()).execute(&mut *tx).await?;
    crate::evidence::record_event_tx(
        &mut tx,
        id,
        "profile_changed",
        json!({"asset":asset,"policy":policy}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"saved":true,"digest":digest})))
}

fn normalize_policy(mut policy: AccessPolicy) -> ApiResult<AccessPolicy> {
    if policy.terminal_accounts.len() > 16
        || policy.services.len() > 64
        || policy.read_directories.len() > 16
        || policy.write_directories.len() > 16
    {
        return Err(ApiError::BadRequest("授权范围数量超限".into()));
    }
    for name in policy.terminal_accounts.iter().chain(&policy.services) {
        if name.is_empty()
            || name.len() > 255
            || name.starts_with('-')
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'@'))
        {
            return Err(ApiError::BadRequest("账号或服务名称无效".into()));
        }
    }
    for path in policy
        .read_directories
        .iter()
        .chain(&policy.write_directories)
    {
        if !path.starts_with('/')
            || path == "/"
            || path.len() > 4096
            || path.contains(['\0', '\n', '\r'])
            || path.split('/').any(|p| p == ".." || p == ".")
        {
            return Err(ApiError::BadRequest(
                "允许目录必须为明确的绝对目录，不能授权根目录".into(),
            ));
        }
    }
    policy.maximum_file_bytes = policy.maximum_file_bytes.clamp(1, 256 * 1024);
    Ok(policy)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleInput {
    lifecycle: String,
    from: Option<i64>,
    until: Option<i64>,
    reason: String,
}
pub async fn lifecycle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<LifecycleInput>,
) -> ApiResult<Json<Value>> {
    crate::control_center::require_server(&state, &headers, id, "servers:write").await?;
    if !["active", "maintenance", "draining", "retired"].contains(&input.lifecycle.as_str())
        || input
            .until
            .is_some_and(|until| until <= input.from.unwrap_or(now_timestamp()))
    {
        return Err(ApiError::BadRequest("生命周期或维护时间范围无效".into()));
    }
    if input.lifecycle == "retired" {
        return Err(ApiError::Conflict(
            "退役必须通过现有设备清理确认流程；先停止接收任务，再检查关联服务并删除设备".into(),
        ));
    }
    let reason = crate::text::bounded(&input.reason, 512)?;
    let mut tx = state.pool.begin().await?;
    crate::servers::lock(&mut tx, id).await?;
    sqlx::query("INSERT INTO fleet_profiles(server_id,lifecycle,maintenance_from,maintenance_until,maintenance_reason,updated_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(server_id) DO UPDATE SET lifecycle=$2,maintenance_from=$3,maintenance_until=$4,maintenance_reason=$5,updated_at=$6")
        .bind(id).bind(&input.lifecycle).bind(input.from).bind(input.until).bind(reason).bind(now_timestamp()).execute(&mut *tx).await?;
    crate::evidence::record_event_tx(
        &mut tx,
        id,
        "lifecycle_changed",
        json!({"lifecycle":input.lifecycle,"from":input.from,"until":input.until}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"saved":true})))
}

/// The runtimes and deployments the traffic service keeps for the server, which a migration
/// leaves behind.
async fn references(pool: &sqlx::PgPool, id: i64) -> ApiResult<Value> {
    let references = crate::server_runtime::references(pool, id).await?;
    Ok(json!({"plugins":references.enabled,"applications":references.deployments}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationInput {
    target_server_id: i64,
    confirmation: Option<String>,
}
async fn preview(state: &AppState, source: i64, target: i64) -> ApiResult<Value> {
    if source == target {
        return Err(ApiError::BadRequest("迁移来源与目标必须不同".into()));
    }
    let rows=sqlx::query("SELECT id,name,agent_settings,device_public_key FROM servers WHERE id=ANY($1) AND deleted_at IS NULL ORDER BY id").bind(vec![source,target]).fetch_all(&state.pool).await?;
    if rows.len() != 2 {
        return Err(ApiError::NotFound);
    }
    let mut telemetry =
        crate::server_observation::telemetry_settings(&state.pool, &[source, target]).await?;
    let probes: Vec<Value> =
        sqlx::query_scalar("SELECT spec FROM network_probes WHERE server_id=$1 ORDER BY id")
            .bind(source)
            .fetch_all(&state.pool)
            .await?;
    let snapshot = json!({"source":source,"target":target,"collection":rows.iter().map(|row|json!({"id":row.get::<i64,_>("id"),"name":row.get::<String,_>("name"),"agent_settings":row.get::<Value,_>("agent_settings"),"telemetry_settings":telemetry.remove(&row.get::<i64,_>("id")),"enrolled":row.get::<Option<String>,_>("device_public_key").is_some()})).collect::<Vec<_>>(),"probes":probes,"related":references(&state.pool,source).await?,"scope":["agent_settings","telemetry_settings","network_probes"],"identity_copied":false,"business_requires_plugin_migration":true});
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&snapshot).map_err(anyhow::Error::from)?)
    );
    Ok(json!({"snapshot":snapshot,"confirmation":digest}))
}
pub async fn migration_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<MigrationInput>,
) -> ApiResult<Json<Value>> {
    crate::control_center::require_server(&state, &headers, id, "servers:read").await?;
    crate::control_center::require_server(&state, &headers, input.target_server_id, "servers:read")
        .await?;
    Ok(Json(preview(&state, id, input.target_server_id).await?))
}
pub async fn migrate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<MigrationInput>,
) -> ApiResult<Json<Value>> {
    crate::control_center::require_server(&state, &headers, id, "servers:write").await?;
    crate::control_center::require_server(
        &state,
        &headers,
        input.target_server_id,
        "servers:write",
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    crate::server_observation::lock_latency_tasks_tx(&mut tx).await?;
    crate::servers::lock_all(
        &mut tx,
        &[id, input.target_server_id],
        crate::servers::RowLock::UpdateRecord,
    )
    .await?;
    crate::device_work::ensure_accepts_tasks_tx(&mut tx, input.target_server_id).await?;
    let current = preview(&state, id, input.target_server_id).await?;
    if input.confirmation.as_deref() != current["confirmation"].as_str() {
        return Err(ApiError::Conflict("迁移影响已变化，请重新预览".into()));
    }
    let occupied: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM network_probes WHERE server_id=$1)")
            .bind(input.target_server_id)
            .fetch_one(&mut *tx)
            .await?;
    if occupied {
        return Err(ApiError::Conflict(
            "目标已有拨测，不能直接覆盖；请先处理目标配置".into(),
        ));
    }
    sqlx::query("UPDATE servers target SET agent_settings=source.agent_settings FROM servers source WHERE target.id=$2 AND source.id=$1")
        .bind(id).bind(input.target_server_id).execute(&mut *tx).await?;
    crate::server_observation::copy_telemetry_settings_tx(&mut tx, id, input.target_server_id)
        .await?;
    for spec in current["snapshot"]["probes"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let mut spec = spec.clone();
        let probe_id = Uuid::new_v4();
        spec["id"] = json!(probe_id);
        sqlx::query("INSERT INTO network_probes(id,server_id,spec) VALUES($1,$2,$3)")
            .bind(probe_id)
            .bind(input.target_server_id)
            .bind(spec)
            .execute(&mut *tx)
            .await?;
    }
    crate::evidence::record_event_tx(
        &mut tx,
        input.target_server_id,
        "collection_migrated",
        json!({"source":id,"identity_copied":false}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"migrated":true,"business_requires_plugin_migration":true}),
    ))
}

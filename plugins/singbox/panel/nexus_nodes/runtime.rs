use super::{
    runtime_model::{RuntimeSnapshot, digest, inspect},
    runtime_publish,
};
use crate::{
    AppState, control_center,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    routing::post,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/nexus-nodes/runtime/preview", post(preview))
        .route("/nexus-nodes/runtime/prepare", post(prepare))
        .route("/nexus-nodes/runtime/{id}/candidate", post(candidate))
        .route("/nexus-nodes/runtime/{id}/activate", post(activate))
        .route("/nexus-nodes/runtime/{id}/pause", post(pause))
        .route("/nexus-nodes/runtime/{id}/discard", post(discard))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    payload: RuntimeSnapshot,
    expected_digest: Option<String>,
}

async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    Ok(Json(inspect(&state, &mut tx, &input.payload).await?))
}

async fn prepare(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    control_center::require_recent_proof(&state, &headers).await?;
    let snapshot = input.payload;
    let hash = digest(&snapshot)?;
    if input.expected_digest.as_deref() != Some(&hash) {
        return Err(ApiError::Conflict("请先预览并确认同一份运行配置".into()));
    }
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    crate::business::lock_server(&mut tx, snapshot.server_id).await?;
    let report = inspect(&state, &mut tx, &snapshot).await?;
    if !report["conflicts"].as_array().unwrap().is_empty() {
        return Err(ApiError::Conflict(
            "运行配置与原资料或目标主机冲突，请重新预览".into(),
        ));
    }
    if report["already_prepared"] != true {
        let credential = control_center::credentials::store_generated_on(
            &mut tx,
            &format!("Nexus runtime {}", snapshot.server_id),
            "backup",
            serde_json::to_value(&snapshot).map_err(anyhow::Error::from)?,
        )
        .await?;
        sqlx::query("INSERT INTO singbox_nexus_runtime_hosts(server_id,source_id,credential_id,digest,created_at) VALUES($1,$2,$3,$4,$5)")
            .bind(snapshot.server_id).bind(&snapshot.source_id).bind(credential).bind(&hash).bind(now_timestamp()).execute(&mut *tx).await?;
        sqlx::query("SELECT set_config('apsp.nexus_runtime_write','yes',TRUE)")
            .execute(&mut *tx)
            .await?;
        for entry in &snapshot.nodes {
            let node = &entry.preserved.node;
            let id: i64 = sqlx::query_scalar("INSERT INTO nodes(name,server_id,protocol,port,public_host,sni,private_key,public_key,short_id,protocol_config,enabled,settings) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING id")
                .bind(&node.name).bind(snapshot.server_id).bind(node.protocol_config.kind()).bind(i32::from(node.port)).bind(&node.public_host).bind(&node.sni).bind(&node.private_key).bind(&node.public_key).bind(&node.short_id)
                .bind(json!(node.protocol_config)).bind(node.enabled).bind(json!(node.settings)).fetch_one(&mut *tx).await?;
            sqlx::query("INSERT INTO singbox_nexus_runtime_nodes(inventory_id,node_id,server_id,legacy_group_ids,rate) SELECT id,$3,server_id,ARRAY(SELECT (g->>'id')::bigint FROM jsonb_array_elements(display->'groups') g),(display->>'rate')::numeric FROM singbox_nexus_nodes WHERE source_id=$1 AND legacy_node_id=$2")
                .bind(&snapshot.source_id).bind(entry.legacy_node_id).bind(id).execute(&mut *tx).await?;
        }
        // An explicit disabled plugin also blocks capability-based auto-enable.
        sqlx::query("INSERT INTO server_plugins(server_id,plugin,enabled,source,enabled_at) VALUES($1,'sing-box',FALSE,'administrator',$2) ON CONFLICT(server_id,plugin) DO UPDATE SET enabled=FALSE,source='administrator'")
            .bind(snapshot.server_id).bind(now_timestamp()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(
        json!({"server_id":snapshot.server_id,"digest":hash,"state":if report["already_prepared"]==true {report["state"].clone()} else {json!("prepared")},"published":false}),
    ))
}

async fn candidate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(server): Path<i64>,
) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    control_center::require_recent_proof(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    crate::business::lock_server(&mut tx, server).await?;
    let candidate = runtime_publish::compile(&state, &mut tx, server).await?;
    // This explicit owner-only export is saved as a private file by the cutover
    // tool. The regular inventory/status API never returns native credentials.
    Ok(Json(
        json!({"payload":{"bundle":candidate.bundle},"bundle_sha256":candidate.hash,"config_sha256":candidate.config_hash}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Switch {
    expected_digest: String,
    config_sha256: String,
    backup_sha256: String,
    stopped_at: i64,
    stopped_services: Vec<String>,
}

fn evidence(input: &Switch) -> ApiResult<Value> {
    let now = now_timestamp();
    if input.stopped_at > now
        || now.saturating_sub(input.stopped_at) > 120
        || input.stopped_services.is_empty()
        || input.stopped_services.len() > 20
        || input.stopped_services.iter().any(|s| {
            s.len() > 100
                || s.is_empty()
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
        })
        || [
            &input.expected_digest,
            &input.config_sha256,
            &input.backup_sha256,
        ]
        .iter()
        .any(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(ApiError::BadRequest(
            "需提供本次主机备份、候选检查和两分钟内停用旧发布器的记录".into(),
        ));
    }
    Ok(
        json!({"config_sha256":input.config_sha256,"backup_sha256":input.backup_sha256,"stopped_at":input.stopped_at,"stopped_services":input.stopped_services}),
    )
}

async fn activate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(server): Path<i64>,
    Json(input): Json<Switch>,
) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    control_center::require_recent_proof(&state, &headers).await?;
    let proof = evidence(&input)?;
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    crate::business::lock_server(&mut tx, server).await?;
    let host = sqlx::query("SELECT h.digest,h.state,s.last_seen,s.static_info,s.capabilities,s.device_public_key FROM singbox_nexus_runtime_hosts h JOIN servers s ON s.id=h.server_id WHERE h.server_id=$1 FOR UPDATE OF h")
        .bind(server).fetch_optional(&mut *tx).await?.ok_or(ApiError::NotFound)?;
    if host.get::<String, _>("digest") != input.expected_digest {
        return Err(ApiError::Conflict("运行快照已改变".into()));
    }
    let info: Value = host.get("static_info");
    let caps: Value = host.get("capabilities");
    let version = info["agent_version"].as_str().unwrap_or_default();
    let parts: Option<Vec<u32>> = version.split('.').map(|v| v.parse().ok()).collect();
    let current = parts.is_some_and(|v| v.len() == 3 && v.as_slice() >= [0, 3, 1].as_slice());
    let online = host
        .get::<Option<i64>, _>("last_seen")
        .is_some_and(|at| (0..=60).contains(&now_timestamp().saturating_sub(at)));
    let capable = caps.as_array().is_some_and(|values| {
        values.iter().any(|v| v == "singbox")
            && values
                .iter()
                .any(|v| v == apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY)
    });
    if !online
        || !current
        || !capable
        || host.get::<Option<String>, _>("device_public_key").is_none()
    {
        return Err(ApiError::Conflict(
            "请先以正式签名的 apsp Agent 0.3.1 或更新版本接入主机并保持在线".into(),
        ));
    }
    crate::agent::runtime_artifact(&state, &info).await?;
    let candidate = runtime_publish::compile(&state, &mut tx, server).await?;
    if candidate.config_hash != input.config_sha256 {
        return Err(ApiError::Conflict(
            "用户授权或候选配置已改变，需重新检查候选后再切换".into(),
        ));
    }
    let bridge: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_nexus_bridge WHERE enabled AND last_sync_at>=$1)",
    )
    .bind(now_timestamp() - 120)
    .fetch_one(&mut *tx)
    .await?;
    if !bridge {
        return Err(ApiError::Conflict(
            "订阅与流量兼容桥未就绪，保留原服务".into(),
        ));
    }
    sqlx::query("UPDATE singbox_nexus_runtime_hosts SET state='active',activated_at=COALESCE(activated_at,$2),evidence=$3 WHERE server_id=$1")
        .bind(server).bind(now_timestamp()).bind(proof).execute(&mut *tx).await?;
    sqlx::query("UPDATE server_plugins SET enabled=TRUE WHERE server_id=$1 AND plugin='sing-box'")
        .bind(server)
        .execute(&mut *tx)
        .await?;
    crate::business::mark_dirty(&mut tx, &[server]).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"server_id":server,"state":"active","device_applied":false}),
    ))
}

async fn pause(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(server): Path<i64>,
    Json(input): Json<Switch>,
) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    control_center::require_recent_proof(&state, &headers).await?;
    let proof = evidence(&input)?;
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    crate::business::lock_server(&mut tx, server).await?;
    let changed = sqlx::query("UPDATE singbox_nexus_runtime_hosts SET state='paused',evidence=$3 WHERE server_id=$1 AND digest=$2")
        .bind(server).bind(input.expected_digest).bind(proof).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(ApiError::Conflict("接管状态已改变".into()));
    }
    sqlx::query("UPDATE server_plugins SET enabled=FALSE WHERE server_id=$1 AND plugin='sing-box'")
        .bind(server)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"server_id":server,"state":"paused","restart_legacy_only_after_agent_stopped":true}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Discard {
    expected_digest: String,
}

/// Remove a reviewed but never activated preparation so a corrected snapshot
/// can be prepared. Hosts that ever published keep their history and can only
/// be paused.
async fn discard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(server): Path<i64>,
    Json(input): Json<Discard>,
) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    control_center::require_recent_proof(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    crate::business::lock_server(&mut tx, server).await?;
    let host = sqlx::query("SELECT digest,state,activated_at,credential_id FROM singbox_nexus_runtime_hosts WHERE server_id=$1 FOR UPDATE")
        .bind(server).fetch_optional(&mut *tx).await?.ok_or(ApiError::NotFound)?;
    if host.get::<String, _>("digest") != input.expected_digest {
        return Err(ApiError::Conflict("运行快照已改变".into()));
    }
    let published: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM deployments WHERE server_id=$1 AND module='singbox')",
    )
    .bind(server)
    .fetch_one(&mut *tx)
    .await?;
    if host.get::<String, _>("state") != "prepared"
        || host.get::<Option<i64>, _>("activated_at").is_some()
        || published
    {
        return Err(ApiError::Conflict(
            "已启动过的接管保留发布与流量记录，只能暂停".into(),
        ));
    }
    sqlx::query("SELECT set_config('apsp.nexus_runtime_write','yes',TRUE)")
        .execute(&mut *tx)
        .await?;
    let nodes: Vec<i64> = sqlx::query_scalar(
        "DELETE FROM singbox_nexus_runtime_nodes WHERE server_id=$1 RETURNING node_id",
    )
    .bind(server)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM nodes WHERE id=ANY($1)")
        .bind(&nodes)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM singbox_nexus_runtime_hosts WHERE server_id=$1")
        .bind(server)
        .execute(&mut *tx)
        .await?;
    // The encrypted snapshot is retired, not reused by a later preparation.
    sqlx::query("UPDATE credential_entries SET enabled=FALSE,updated_at=$2 WHERE id=$1")
        .bind(host.get::<uuid::Uuid, _>("credential_id"))
        .bind(now_timestamp())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"server_id":server,"state":"pending_agent","discarded_nodes":nodes.len()}),
    ))
}

use super::{
    runtime_model::{Revision, RuntimeNode, RuntimeSnapshot, digest, inspect, inspect_extension},
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
use sqlx::{Postgres, Row, Transaction};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/nexus-nodes/runtime/preview", post(preview))
        .route("/nexus-nodes/runtime/prepare", post(prepare))
        .route("/nexus-nodes/runtime/{id}/candidate", post(candidate))
        .route("/nexus-nodes/runtime/{id}/activate", post(activate))
        .route("/nexus-nodes/runtime/{id}/pause", post(pause))
        .route("/nexus-nodes/runtime/{id}/discard", post(discard))
        .route(
            "/nexus-nodes/runtime/{id}/extend/preview",
            post(extend_preview),
        )
        .route("/nexus-nodes/runtime/{id}/extend", post(extend))
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
        insert_lines(
            &mut tx,
            &snapshot.source_id,
            snapshot.server_id,
            &snapshot.nodes,
        )
        .await?;
        // An explicit disabled plugin also blocks capability-based auto-enable.
        sqlx::query("INSERT INTO server_plugins(server_id,plugin,enabled,source,enabled_at) VALUES($1,'sing-box',FALSE,'administrator',$2) ON CONFLICT(server_id,plugin) DO UPDATE SET enabled=FALSE,source='administrator'")
            .bind(snapshot.server_id).bind(now_timestamp()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(
        json!({"server_id":snapshot.server_id,"digest":hash,"state":if report["already_prepared"]==true {report["state"].clone()} else {json!("prepared")},"published":false}),
    ))
}

/// Native rows for preserved lines, linked to their inventory and to the policy
/// groups of their original groups (ADR 0115). Caller enables the runtime write.
async fn insert_lines(
    tx: &mut Transaction<'_, Postgres>,
    source: &str,
    server: i64,
    entries: &[RuntimeNode],
) -> ApiResult<()> {
    for entry in entries {
        let node = &entry.preserved.node;
        let id: i64 = sqlx::query_scalar("INSERT INTO nodes(name,server_id,protocol,port,public_host,sni,private_key,public_key,short_id,protocol_config,enabled,settings) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING id")
            .bind(&node.name).bind(server).bind(node.protocol_config.kind()).bind(i32::from(node.port)).bind(&node.public_host).bind(&node.sni).bind(&node.private_key).bind(&node.public_key).bind(&node.short_id)
            .bind(json!(node.protocol_config)).bind(node.enabled).bind(json!(node.settings)).fetch_one(&mut **tx).await?;
        sqlx::query("INSERT INTO singbox_nexus_runtime_nodes(inventory_id,node_id,server_id,legacy_group_ids,rate) SELECT id,$3,server_id,ARRAY(SELECT (g->>'id')::bigint FROM jsonb_array_elements(display->'groups') g),(display->>'rate')::numeric FROM singbox_nexus_nodes WHERE source_id=$1 AND legacy_node_id=$2")
            .bind(source).bind(entry.legacy_node_id).bind(id).execute(&mut **tx).await?;
        sqlx::query("INSERT INTO singbox_policy_nodes(group_id,node_id) SELECT singbox_legacy_policy_group(g),node_id FROM singbox_nexus_runtime_nodes, unnest(legacy_group_ids) g WHERE node_id=$1 ON CONFLICT DO NOTHING")
            .bind(id).execute(&mut **tx).await?;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Extension {
    /// Digest of the host's current snapshot, as the panel stored it.
    expected_digest: String,
    /// Digest of the merged snapshot from the preview; required to commit.
    #[serde(default)]
    extended_digest: Option<String>,
    /// Lines to add (whole snapshot entries).
    #[serde(default)]
    nodes: Vec<RuntimeNode>,
    /// Lines already in the snapshot whose native configuration is replaced.
    #[serde(default)]
    configurations: Vec<Revision>,
    /// Services stopped on the host to free the added listeners; required to commit.
    #[serde(default)]
    stopped_services: Vec<String>,
    #[serde(default)]
    stopped_at: Option<i64>,
}

/// The stored snapshot of a host that already has one, locked for the change.
async fn stored_snapshot(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    expected: &str,
) -> ApiResult<(RuntimeSnapshot, String, uuid::Uuid)> {
    let host = sqlx::query("SELECT digest,state,credential_id FROM singbox_nexus_runtime_hosts WHERE server_id=$1 FOR UPDATE")
        .bind(server).fetch_optional(&mut **tx).await?.ok_or(ApiError::NotFound)?;
    if host.get::<String, _>("digest") != expected {
        return Err(ApiError::Conflict("运行快照已改变".into()));
    }
    let credential: uuid::Uuid = host.get("credential_id");
    let raw = control_center::credentials::resolve_reference(
        state,
        credential,
        "backup",
        "nexus-runtime-extension",
    )
    .await?;
    let snapshot: RuntimeSnapshot = serde_json::from_value(raw).map_err(anyhow::Error::from)?;
    Ok((snapshot, host.get("state"), credential))
}

/// Previews added lines: archive checks for the new entries and the candidate
/// the host would receive, compiled in a transaction that is then discarded.
async fn extend_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(server): Path<i64>,
    Json(input): Json<Extension>,
) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    control_center::require_recent_proof(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    crate::business::lock_server(&mut tx, server).await?;
    let (stored, _, _) = stored_snapshot(&state, &mut tx, server, &input.expected_digest).await?;
    let (merged, mut report) = inspect_extension(
        &state,
        &mut tx,
        &stored,
        &input.nodes,
        &input.configurations,
    )
    .await?;
    if report["conflicts"].as_array().is_some_and(|c| c.is_empty()) {
        sqlx::query("SELECT set_config('apsp.nexus_runtime_write','yes',TRUE)")
            .execute(&mut *tx)
            .await?;
        insert_lines(&mut tx, &merged.source_id, server, &input.nodes).await?;
        let candidate = runtime_publish::compile_snapshot(&mut tx, server, merged).await?;
        report["candidate"] = json!({"payload":{"bundle":candidate.bundle,"bundle_sha256":candidate.hash},"config_sha256":candidate.config_hash});
    }
    tx.rollback().await?;
    Ok(Json(report))
}

/// Stores the merged snapshot and the added lines; an active host is published again.
async fn extend(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(server): Path<i64>,
    Json(input): Json<Extension>,
) -> ApiResult<Json<Value>> {
    let actor = control_center::require_owner(&state, &headers).await?;
    control_center::require_recent_proof(&state, &headers).await?;
    let now = now_timestamp();
    // Only an added listener needs its legacy service stopped first.
    if !input.nodes.is_empty()
        && (input
            .stopped_at
            .is_none_or(|at| at > now || now.saturating_sub(at) > 600)
            || input.stopped_services.is_empty()
            || input.stopped_services.len() > 20
            || input.stopped_services.iter().any(|s| {
                s.len() > 100
                    || s.is_empty()
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
            }))
    {
        return Err(ApiError::BadRequest(
            "需提供十分钟内为新增监听停用的旧服务记录".into(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    crate::business::lock_server(&mut tx, server).await?;
    let (stored, host_state, previous) =
        stored_snapshot(&state, &mut tx, server, &input.expected_digest).await?;
    let (merged, report) = inspect_extension(
        &state,
        &mut tx,
        &stored,
        &input.nodes,
        &input.configurations,
    )
    .await?;
    if !report["conflicts"].as_array().is_some_and(|c| c.is_empty()) {
        return Err(ApiError::Conflict(
            "追加的线路与原资料或目标主机冲突，请重新预览".into(),
        ));
    }
    let hash = digest(&merged)?;
    if input.extended_digest.as_deref() != Some(hash.as_str()) {
        return Err(ApiError::Conflict("请先预览并确认同一份追加配置".into()));
    }
    let credential = control_center::credentials::store_generated_on(
        &mut tx,
        &format!("Nexus runtime {server}"),
        "backup",
        serde_json::to_value(&merged).map_err(anyhow::Error::from)?,
    )
    .await?;
    sqlx::query(
        "UPDATE singbox_nexus_runtime_hosts SET credential_id=$2,digest=$3 WHERE server_id=$1",
    )
    .bind(server)
    .bind(credential)
    .bind(&hash)
    .execute(&mut *tx)
    .await?;
    // The replaced snapshot is a subset of the new one; it is retired, not reused.
    sqlx::query("UPDATE credential_entries SET enabled=FALSE,updated_at=$2 WHERE id=$1")
        .bind(previous)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT set_config('apsp.nexus_runtime_write','yes',TRUE)")
        .execute(&mut *tx)
        .await?;
    insert_lines(&mut tx, &merged.source_id, server, &input.nodes).await?;
    sqlx::query("SELECT set_config('apsp.nexus_runtime_write','',TRUE)")
        .execute(&mut *tx)
        .await?;
    let republish = host_state == "active";
    if republish {
        crate::business::mark_dirty(&mut tx, &[server]).await?;
    }
    let added: Vec<i64> = input.nodes.iter().map(|e| e.legacy_node_id).collect();
    let revised: Vec<i64> = input
        .configurations
        .iter()
        .map(|r| r.legacy_node_id)
        .collect();
    sqlx::query("INSERT INTO management_audit(admin_id,action,object_path,request_diff,result,occurred_at) VALUES($1,'nexus.runtime.extend',$2,$3,$4,$5)")
        .bind(actor)
        .bind(format!("/api/plugins/sing-box/nexus-nodes/runtime/{server}/extend"))
        .bind(json!({"from":input.expected_digest,"to":hash,"added_legacy_nodes":added,"revised_legacy_nodes":revised,"stopped_services":input.stopped_services,"stopped_at":input.stopped_at}))
        .bind(json!({"state":"recorded","republish":republish}))
        .bind(now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"server_id":server,"digest":hash,"added":added,"revised":revised,"state":host_state,"republish":republish}),
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
    /// Skips the one-host-per-day pacing on activation. Ignored when pausing.
    #[serde(default)]
    ignore_stagger: bool,
    /// Activates even though the host's address is measured as blocked from
    /// China. Ignored when pausing.
    #[serde(default)]
    ignore_reachability: bool,
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

/// Moving a whole fleet at once leaves nothing to compare against and nothing
/// to fall back on if a change is detected: on 2026-10-26 eleven hosts cut over
/// within hours were blocked together.
const CUTOVER_SPACING_SECS: i64 = 24 * 3600;

async fn stagger(tx: &mut Transaction<'_, Postgres>, server: i64, now: i64) -> ApiResult<()> {
    let recent: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(last_activated_at) FROM singbox_nexus_runtime_hosts WHERE server_id<>$1 AND state='active'",
    )
    .bind(server)
    .fetch_one(&mut **tx)
    .await?;
    if let Some(remaining) = recent
        .map(|at| at + CUTOVER_SPACING_SECS - now)
        .filter(|remaining| *remaining > 0)
    {
        return Err(ApiError::Conflict(format!(
            "上一台主机切换未满 24 小时，还需等待约 {} 小时；观察国内可达性后再继续，确需提前可明确选择跳过间隔",
            remaining / 3600 + 1
        )));
    }
    Ok(())
}

/// A measurement older than this says nothing: the monitor may be off or deferring.
const REACHABILITY_FRESH_SECS: i64 = 6 * 3600;

/// Cutting over a host that is already blocked from China would fail for
/// members and read as a runtime fault. An unknown or stale state does not stop
/// the cutover, because the monitor is opt-in.
async fn reachable(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    now: i64,
) -> ApiResult<()> {
    let nodes: Vec<i64> = sqlx::query_scalar(
        "SELECT inventory_id FROM singbox_nexus_runtime_nodes WHERE server_id=$1 ORDER BY inventory_id",
    )
    .bind(server)
    .fetch_all(&mut **tx)
    .await?;
    for node in nodes {
        let latest = crate::reachability::latest_node_state(
            &state.pool,
            crate::reachability::NodeReference::Nexus(node),
        )
        .await?;
        if latest.is_some_and(|latest| {
            latest.blocked
                && latest
                    .checked_at
                    .is_some_and(|at| now.saturating_sub(at) <= REACHABILITY_FRESH_SECS)
        }) {
            return Err(ApiError::Conflict(
                "该主机的公网地址最近测得国内不可达，切换后成员仍无法连接；请先更换地址，确需继续可明确选择跳过检查".into(),
            ));
        }
    }
    Ok(())
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
    let current = parts.is_some_and(|v| v.len() == 3 && v.as_slice() >= [0, 3, 4].as_slice());
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
            "请先以正式签名的 apsp Agent 0.3.4 或更新版本接入主机并保持在线（旧版不接受自维护运行时）".into(),
        ));
    }
    let (limited, synchronized) =
        super::sessions::limits_on_host(&mut tx, server, now_timestamp()).await?;
    if !synchronized {
        return Err(ApiError::Conflict(
            "兼容桥尚未同步旧面板的设备数限制，保留原服务".into(),
        ));
    }
    let controls_sessions = caps.as_array().is_some_and(|values| {
        values
            .iter()
            .any(|v| v == apsp_protocol::RUNTIME_ADMISSION_CAPABILITY)
    });
    if limited > 0 && !controls_sessions {
        return Err(ApiError::Conflict(format!(
            "该主机有 {limited} 名设定设备数限制的成员；需要提供会话控制能力的 Agent 并安装运行时日志命名空间，保留原服务"
        )));
    }
    if !input.ignore_stagger {
        stagger(&mut tx, server, now_timestamp()).await?;
    }
    if !input.ignore_reachability {
        reachable(&state, &mut tx, server, now_timestamp()).await?;
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
    sqlx::query("UPDATE singbox_nexus_runtime_hosts SET state='active',activated_at=COALESCE(activated_at,$2),last_activated_at=$2,evidence=$3 WHERE server_id=$1")
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
    sqlx::query("DELETE FROM singbox_policy_nodes WHERE node_id=ANY($1)")
        .bind(&nodes)
        .execute(&mut *tx)
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

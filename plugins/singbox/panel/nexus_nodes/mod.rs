//! Credential-preserving inventory import. No runtime or grant mutation occurs.
mod model;
mod runtime;
mod runtime_model;
pub(crate) mod runtime_publish;
pub(crate) mod sessions;
pub(crate) mod topology;

use crate::{
    AppState, control_center,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::HeaderMap,
    routing::{get, patch, post},
};
use model::{Snapshot, digest, prepare, source_tag};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/nexus-nodes", get(list))
        .route("/nexus-nodes/preview", post(preview))
        .route("/nexus-nodes/import", post(import))
        .route("/nexus-nodes/append/preview", post(append_preview))
        .route("/nexus-nodes/append", post(append))
        .route("/nexus-nodes/{id}/public-endpoint", patch(public_endpoint))
        .route("/nexus-nodes/{id}/groups", patch(legacy_groups))
        .route("/nexus-nodes/{id}/retire", post(retire))
        .merge(runtime::router())
        .layer(DefaultBodyLimit::max(3 * 1024 * 1024))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicEndpoint {
    public_host: String,
    public_port: u16,
}

fn valid_public_host(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
        || (!host.is_empty()
            && host.len() <= 253
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            }))
}

/// Records where clients reach an imported node after its IP or public port
/// moved. Only the catalog changes: the listener, keys and client identities
/// stay preserved, so a port-forward in front of the host keeps working.
async fn public_endpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<PublicEndpoint>,
) -> ApiResult<Json<Value>> {
    let actor = control_center::require_owner(&state, &headers).await?;
    control_center::require_recent_proof(&state, &headers).await?;
    if !valid_public_host(&input.public_host) || input.public_port == 0 {
        return Err(ApiError::BadRequest("公网地址或端口无效".into()));
    }
    let mut tx = state.pool.begin().await?;
    let before: Value =
        sqlx::query_scalar("SELECT display FROM singbox_nexus_nodes WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    let port = input.public_port.to_string();
    sqlx::query(
        "UPDATE singbox_nexus_nodes SET display=display||jsonb_build_object('public_host',$2::text,'public_port',$3::text) WHERE id=$1",
    )
    .bind(id)
    .bind(&input.public_host)
    .bind(&port)
    .execute(&mut *tx)
    .await?;
    sqlx::query("SELECT set_config('apsp.nexus_runtime_write','yes',TRUE)")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE nodes SET public_host=$2 WHERE id=(SELECT node_id FROM singbox_nexus_runtime_nodes WHERE inventory_id=$1)",
    )
    .bind(id)
    .bind(&input.public_host)
    .execute(&mut *tx)
    .await?;
    sqlx::query("SELECT set_config('apsp.nexus_runtime_write','',TRUE)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO management_audit(admin_id,action,object_path,request_diff,result,occurred_at) VALUES($1,'nexus.nodes.public_endpoint',$2,$3,$4,$5)")
        .bind(actor)
        .bind(format!("/api/plugins/sing-box/nexus-nodes/{id}/public-endpoint"))
        .bind(json!({"from":{"public_host":before["public_host"],"public_port":before["public_port"]},"to":{"public_host":input.public_host,"public_port":port}}))
        .bind(json!({"state":"recorded"}))
        .bind(apsp_protocol::now_timestamp())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id":id,"public_host":input.public_host,"public_port":port}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyGroup {
    id: i64,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyGroups {
    groups: Vec<LegacyGroup>,
}

/// Records a permission group the source added to a node after import, so a
/// later cutover does not drop its members. Keys and client identities stay as
/// preserved; only who may use the node changes.
async fn legacy_groups(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<LegacyGroups>,
) -> ApiResult<Json<Value>> {
    let actor = control_center::require_owner(&state, &headers).await?;
    control_center::require_recent_proof(&state, &headers).await?;
    let ids: Vec<i64> = input.groups.iter().map(|group| group.id).collect();
    let distinct: std::collections::BTreeSet<i64> = ids.iter().copied().collect();
    if ids.is_empty()
        || distinct.len() != ids.len()
        || ids.iter().any(|id| *id <= 0)
        || input
            .groups
            .iter()
            .any(|group| group.name.is_empty() || group.name.len() > 512)
    {
        return Err(ApiError::BadRequest("权限组无效".into()));
    }
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let server: Option<i64> =
        sqlx::query_scalar("SELECT server_id FROM singbox_nexus_nodes WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    if let Some(server) = server {
        crate::business::lock_server(&mut tx, server).await?;
    }
    let before: Value =
        sqlx::query_scalar("SELECT display FROM singbox_nexus_nodes WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    let mapped: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT p.legacy_group_id) FROM singbox_nexus_plan_mappings p JOIN singbox_nexus_bridge b ON b.import_id=p.import_id WHERE p.legacy_group_id=ANY($1)")
        .bind(&ids)
        .fetch_one(&mut *tx)
        .await?;
    if mapped != ids.len() as i64 {
        return Err(ApiError::Conflict(
            "权限组还没有对应的套餐，成员无法继承".into(),
        ));
    }
    let views: Vec<Value> = input
        .groups
        .iter()
        .map(|group| json!({"id":group.id,"name":group.name}))
        .collect();
    sqlx::query("UPDATE singbox_nexus_nodes SET display=display||jsonb_build_object('groups',$2::jsonb) WHERE id=$1")
        .bind(id)
        .bind(json!(views))
        .execute(&mut *tx)
        .await?;
    let state_after: Option<String> = sqlx::query_scalar("UPDATE singbox_nexus_runtime_nodes r SET legacy_group_ids=$2 FROM singbox_nexus_runtime_hosts h WHERE r.inventory_id=$1 AND h.server_id=r.server_id RETURNING h.state")
        .bind(id)
        .bind(&ids)
        .fetch_optional(&mut *tx)
        .await?;
    if let (Some(server), Some("active")) = (server, state_after.as_deref()) {
        crate::business::mark_dirty(&mut tx, &[server]).await?;
    }
    sqlx::query("INSERT INTO management_audit(admin_id,action,object_path,request_diff,result,occurred_at) VALUES($1,'nexus_node_groups',$2,$3,$4,$5)")
        .bind(actor)
        .bind(format!("/api/plugins/sing-box/nexus-nodes/{id}/groups"))
        .bind(json!({"from":before["groups"],"to":views}))
        .bind(json!({"state":"recorded"}))
        .bind(now_timestamp())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"groups":views})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Retire {
    /// The node number on the source, repeated so that a mistyped inventory
    /// id cannot remove a different line.
    legacy_node_id: i64,
}

/// Removes an imported line for good, also from a host that already runs the
/// preserved configuration. The encrypted runtime snapshot keeps the entry as
/// history; publication skips entries whose inventory row is gone, so the
/// host is republished without that listener. Recorded usage stays. The
/// source panel's own row is removed separately, after this call, because
/// the compatibility bridge expects every taken-over node to exist there.
async fn retire(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<Retire>,
) -> ApiResult<Json<Value>> {
    let actor = control_center::require_owner(&state, &headers).await?;
    control_center::require_recent_proof(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let server: i64 = sqlx::query_scalar("SELECT server_id FROM singbox_nexus_nodes WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    crate::business::lock_server(&mut tx, server).await?;
    let row = sqlx::query(
        "SELECT source_id,legacy_node_id,credential_id,display FROM singbox_nexus_nodes WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let legacy: i64 = row.get("legacy_node_id");
    if legacy != input.legacy_node_id {
        return Err(ApiError::Conflict("原节点编号与该线路不符，未下线".into()));
    }
    let source: String = row.get("source_id");
    let display: Value = row.get("display");
    let runtime: Option<(i64, String)> = sqlx::query_as(
        "SELECT r.node_id,h.state FROM singbox_nexus_runtime_nodes r JOIN singbox_nexus_runtime_hosts h ON h.server_id=r.server_id WHERE r.inventory_id=$1",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let mut republish = false;
    if let Some((node, host_state)) = &runtime {
        let lines: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM singbox_nexus_runtime_nodes WHERE server_id=$1",
        )
        .bind(server)
        .fetch_one(&mut *tx)
        .await?;
        if lines <= 1 {
            return Err(ApiError::Conflict(
                "这是该主机接管的最后一条线路；整台主机停用请暂停接管".into(),
            ));
        }
        sqlx::query("SELECT set_config('apsp.nexus_runtime_write','yes',TRUE)")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM singbox_nexus_runtime_nodes WHERE inventory_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        // Usage history still refers to the native row, so it is retired, not deleted.
        sqlx::query(
            "UPDATE nodes SET enabled=FALSE,deleted_at=COALESCE(deleted_at,$2) WHERE id=$1",
        )
        .bind(node)
        .bind(now_timestamp())
        .execute(&mut *tx)
        .await?;
        sqlx::query("SELECT set_config('apsp.nexus_runtime_write','',TRUE)")
            .execute(&mut *tx)
            .await?;
        if host_state == "active" {
            crate::business::mark_dirty(&mut tx, &[server]).await?;
            republish = true;
        }
    }
    sqlx::query("DELETE FROM singbox_nexus_nodes WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    // Keep the import's record count matching its rows for later previews.
    sqlx::query(
        "UPDATE singbox_nexus_node_imports SET node_count=node_count-1 WHERE source_id=$1 AND node_count>1",
    )
    .bind(&source)
    .execute(&mut *tx)
    .await?;
    // The line's own preserved configuration (keys, landing credentials) is retired with it.
    sqlx::query("UPDATE credential_entries SET enabled=FALSE,updated_at=$2 WHERE id=$1")
        .bind(row.get::<uuid::Uuid, _>("credential_id"))
        .bind(now_timestamp())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO management_audit(admin_id,action,object_path,request_diff,result,occurred_at) VALUES($1,'nexus.nodes.retire',$2,$3,$4,$5)")
        .bind(actor)
        .bind(format!("/api/plugins/sing-box/nexus-nodes/{id}/retire"))
        .bind(json!({"legacy_node_id":legacy,"server_id":server,"name":display["name"]}))
        .bind(json!({"state":"retired","runtime_node_id":runtime.as_ref().map(|(node, _)| *node),"republish":republish}))
        .bind(now_timestamp())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({
        "id": id,
        "legacy_node_id": legacy,
        "server_id": server,
        "runtime_node_id": runtime.map(|(node, _)| node),
        "republish": republish,
    })))
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Vec<Value>>> {
    control_center::require_owner(&state, &headers).await?;
    let rows = sqlx::query("SELECT n.id,n.source_id,n.legacy_node_id,n.legacy_machine_id,n.legacy_host_id,n.server_id,n.display,s.name AS server_name,i.exported_at,i.imported_at, h.state AS runtime_state, r.node_id AS native_node_id, COALESCE(m.healthy AND m.applied_rev=m.target_rev AND m.applied_rev>0,FALSE) AS runtime_healthy FROM singbox_nexus_nodes n JOIN servers s ON s.id=n.server_id JOIN singbox_nexus_node_imports i ON i.source_id=n.source_id LEFT JOIN singbox_nexus_runtime_hosts h ON h.server_id=n.server_id LEFT JOIN singbox_nexus_runtime_nodes r ON r.inventory_id=n.id LEFT JOIN server_module_status m ON m.server_id=n.server_id AND m.module='singbox' ORDER BY (n.display->>'sort_order')::bigint,n.source_id,n.legacy_node_id")
        .fetch_all(&state.pool).await?;
    let nodes = rows
        .iter()
        .map(|row| {
            let mut value: Value = row.get("display");
            for field in [
                "id",
                "legacy_node_id",
                "legacy_machine_id",
                "legacy_host_id",
                "server_id",
                "exported_at",
                "imported_at",
            ] {
                value[field] = json!(row.get::<i64, _>(field));
            }
            value["state"] = json!(
                match row.get::<Option<String>, _>("runtime_state").as_deref() {
                    Some("active") if row.get::<bool, _>("runtime_healthy") => "applied",
                    Some("active") => "applying",
                    Some("prepared") => "prepared",
                    Some("paused") => "paused",
                    _ => "pending_agent",
                }
            );
            value["native_node_id"] = json!(row.get::<Option<i64>, _>("native_node_id"));
            value["source_id"] = json!(row.get::<String, _>("source_id"));
            value["server_name"] = json!(row.get::<String, _>("server_name"));
            value
        })
        .collect();
    Ok(Json(nodes))
}

/// Encrypts each original node record and adds its inventory row.
async fn store_nodes(
    tx: &mut Transaction<'_, Postgres>,
    source: &str,
    nodes: Vec<model::PreparedNode>,
) -> ApiResult<()> {
    for node in nodes {
        let credential = control_center::credentials::store_generated_on(
            tx,
            &format!("Nexus {source} node {}", node.legacy_id),
            "backup",
            node.archive,
        )
        .await?;
        sqlx::query("INSERT INTO singbox_nexus_nodes(source_id,legacy_node_id,legacy_machine_id,legacy_host_id,server_id,credential_id,display) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(source).bind(node.legacy_id).bind(node.machine_id).bind(node.host_id).bind(node.server_id).bind(credential).bind(node.display).execute(&mut **tx).await?;
    }
    Ok(())
}

/// Checks that every host in the snapshot maps, by its source tag alone, to
/// exactly the server the snapshot names. Locks the server rows it reads.
async fn host_conflicts(
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &Snapshot,
) -> ApiResult<Vec<String>> {
    let servers: Vec<(i64, Value)> = sqlx::query_as(
        "SELECT id,asset_settings FROM servers WHERE deleted_at IS NULL ORDER BY id FOR SHARE",
    )
    .fetch_all(&mut **tx)
    .await?;
    let mut conflicts = Vec::new();
    for host in &snapshot.hosts {
        let tag = source_tag(&snapshot.source_id, host.legacy_id);
        let matches: Vec<_> = servers
            .iter()
            .filter(|(_, settings)| {
                settings["tags"]
                    .as_array()
                    .is_some_and(|tags| tags.iter().any(|v| v.as_str() == Some(&tag)))
            })
            .map(|(id, _)| *id)
            .collect();
        if matches != [host.server_id] {
            conflicts.push(format!(
                "原主机 {} 的来源标签与目标服务器映射不一致",
                host.legacy_id
            ));
        }
    }
    Ok(conflicts)
}

/// An append adds nodes the source gained after its first import without
/// touching what was imported then. Every requested node must be new, or, for a
/// retried request, already present with the same host mapping.
async fn inspect_append(
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &Snapshot,
    nodes: &[model::PreparedNode],
    digest: &str,
) -> ApiResult<Value> {
    let mut conflicts = Vec::new();
    let imported: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_nexus_node_imports WHERE source_id=$1)",
    )
    .bind(&snapshot.source_id)
    .fetch_one(&mut **tx)
    .await?;
    if !imported {
        conflicts.push("来源尚未完整导入，不能追加节点".to_owned());
    }
    let (mut present, mut fresh) = (0, Vec::new());
    for node in nodes {
        let saved: Option<(i64, i64, i64)> = sqlx::query_as("SELECT legacy_machine_id,legacy_host_id,server_id FROM singbox_nexus_nodes WHERE source_id=$1 AND legacy_node_id=$2")
            .bind(&snapshot.source_id).bind(node.legacy_id).fetch_optional(&mut **tx).await?;
        match saved {
            None => fresh.push(node.legacy_id),
            Some(row) if row == (node.machine_id, node.host_id, node.server_id) => present += 1,
            Some(_) => conflicts.push(format!(
                "原节点 {} 已导入且归属不同，不能追加",
                node.legacy_id
            )),
        }
    }
    if present > 0 && !fresh.is_empty() {
        conflicts.push("追加内容与已导入节点部分重叠，请只提交新节点".to_owned());
    }
    conflicts.extend(host_conflicts(tx, snapshot).await?);
    Ok(
        json!({"digest":digest,"node_count":nodes.len(),"new_nodes":fresh,
        "already_imported":present == nodes.len(),"conflicts":conflicts,"state":"pending_agent"}),
    )
}

async fn append_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Preview>,
) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    let snapshot = input.payload;
    let nodes = prepare(&snapshot)?;
    let mut tx = state.pool.begin().await?;
    Ok(Json(
        inspect_append(&mut tx, &snapshot, &nodes, &digest(&snapshot)?).await?,
    ))
}

async fn append(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Import>,
) -> ApiResult<Json<Value>> {
    let actor = control_center::require_owner(&state, &headers).await?;
    control_center::require_recent_proof(&state, &headers).await?;
    let snapshot = input.payload;
    let nodes = prepare(&snapshot)?;
    let digest = digest(&snapshot)?;
    if !input.confirm_staged || input.expected_digest != digest {
        return Err(ApiError::Conflict("请先预览并确认同一份追加快照".into()));
    }
    let mut tx = state.pool.begin().await?;
    // Same lock as a full import: one inventory writer at a time.
    sqlx::query("SELECT pg_advisory_xact_lock(71410071)")
        .execute(&mut *tx)
        .await?;
    let report = inspect_append(&mut tx, &snapshot, &nodes, &digest).await?;
    if !report["conflicts"].as_array().unwrap().is_empty() {
        return Err(ApiError::Conflict(
            "追加节点与已有记录或主机映射冲突，请重新预览".into(),
        ));
    }
    if report["already_imported"] != true {
        let added = nodes.len() as i32;
        store_nodes(&mut tx, &snapshot.source_id, nodes).await?;
        // Keep the source's record count matching its rows for later previews.
        sqlx::query(
            "UPDATE singbox_nexus_node_imports SET node_count=node_count+$2 WHERE source_id=$1",
        )
        .bind(&snapshot.source_id)
        .bind(added)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO management_audit(admin_id,action,object_path,request_diff,result,occurred_at) VALUES($1,'nexus.nodes.append','/api/plugins/sing-box/nexus-nodes/append',$2,$3,$4)")
            .bind(actor).bind(json!({"source_id":snapshot.source_id,"digest":digest,"infrastructure_digest":snapshot.infrastructure_digest}))
            .bind(json!({"added":report["new_nodes"],"state":"pending_agent"})).bind(now_timestamp()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(report))
}

async fn inspect(
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &Snapshot,
    digest: &str,
) -> ApiResult<Value> {
    let existing: Option<(String, i32)> = sqlx::query_as(
        "SELECT digest,node_count FROM singbox_nexus_node_imports WHERE source_id=$1",
    )
    .bind(&snapshot.source_id)
    .fetch_optional(&mut **tx)
    .await?;
    let mut conflicts = Vec::new();
    let already_imported = existing.as_ref().is_some_and(|(saved, _)| saved == digest);
    if existing.as_ref().is_some_and(|(saved, _)| saved != digest) {
        conflicts.push("来源已导入另一份快照，不能覆盖已保存配置".to_owned());
    }
    if let Some((_, count)) = existing {
        let saved: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM singbox_nexus_nodes WHERE source_id=$1")
                .bind(&snapshot.source_id)
                .fetch_one(&mut **tx)
                .await?;
        if saved != i64::from(count) {
            conflicts.push("已导入节点记录不完整，需先恢复原导入".into());
        }
    }
    // Source tags are authoritative, not caller-supplied server ids.
    conflicts.extend(host_conflicts(tx, snapshot).await?);
    Ok(
        json!({"digest":digest,"node_count":snapshot.nodes.len(),"host_count":snapshot.hosts.len(),"machine_count":snapshot.machines.len(),"already_imported":already_imported,"conflicts":conflicts,"state":"pending_agent"}),
    )
}

async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Preview>,
) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    let snapshot = input.payload;
    prepare(&snapshot)?;
    let mut tx = state.pool.begin().await?;
    Ok(Json(
        inspect(&mut tx, &snapshot, &digest(&snapshot)?).await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    // The host redacts this entire envelope, including JSON encoded in strings.
    payload: Snapshot,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Import {
    payload: Snapshot,
    expected_digest: String,
    confirm_staged: bool,
}

async fn import(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Import>,
) -> ApiResult<Json<Value>> {
    let actor = control_center::require_owner(&state, &headers).await?;
    control_center::require_recent_proof(&state, &headers).await?;
    let snapshot = input.payload;
    let nodes = prepare(&snapshot)?;
    let digest = digest(&snapshot)?;
    if !input.confirm_staged || input.expected_digest != digest {
        return Err(ApiError::Conflict("请先预览并确认同一份节点快照".into()));
    }
    let mut tx = state.pool.begin().await?;
    // Serialize imports across requests, including ambiguous HTTP retries.
    sqlx::query("SELECT pg_advisory_xact_lock(71410071)")
        .execute(&mut *tx)
        .await?;
    let report = inspect(&mut tx, &snapshot, &digest).await?;
    if !report["conflicts"].as_array().unwrap().is_empty() {
        return Err(ApiError::Conflict(
            "节点来源或主机映射冲突，请重新预览".into(),
        ));
    }
    if report["already_imported"] != true {
        sqlx::query("INSERT INTO singbox_nexus_node_imports(source_id,digest,infrastructure_digest,exported_at,imported_at,node_count) VALUES($1,$2,$3,$4,$5,$6)").bind(&snapshot.source_id).bind(&digest).bind(&snapshot.infrastructure_digest).bind(snapshot.exported_at).bind(now_timestamp()).bind(nodes.len() as i32).execute(&mut *tx).await?;
        store_nodes(&mut tx, &snapshot.source_id, nodes).await?;
        sqlx::query("INSERT INTO management_audit(admin_id,action,object_path,request_diff,result,occurred_at) VALUES($1,'nexus.nodes.import','/api/plugins/sing-box/nexus-nodes',$2,$3,$4)")
            .bind(actor).bind(json!({"source_id":snapshot.source_id,"digest":digest})).bind(json!({"node_count":snapshot.nodes.len(),"state":"pending_agent"})).bind(now_timestamp()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(report))
}

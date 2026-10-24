//! Credential-preserving inventory import. No runtime or grant mutation occurs.
mod model;
mod runtime;
mod runtime_model;
pub(crate) mod runtime_publish;

use crate::{
    AppState, control_center,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{get, post},
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
        .merge(runtime::router())
        .layer(DefaultBodyLimit::max(3 * 1024 * 1024))
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
    // Source tags are authoritative, not caller-supplied server ids. Lock all
    // host rows to prevent a concurrent mapping edit during the transaction.
    let servers: Vec<(i64, Value)> = sqlx::query_as(
        "SELECT id,asset_settings FROM servers WHERE deleted_at IS NULL ORDER BY id FOR SHARE",
    )
    .fetch_all(&mut **tx)
    .await?;
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
        for node in nodes {
            let credential = control_center::credentials::store_generated_on(
                &mut tx,
                &format!("Nexus {} node {}", snapshot.source_id, node.legacy_id),
                "backup",
                node.archive,
            )
            .await?;
            sqlx::query("INSERT INTO singbox_nexus_nodes(source_id,legacy_node_id,legacy_machine_id,legacy_host_id,server_id,credential_id,display) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(&snapshot.source_id).bind(node.legacy_id).bind(node.machine_id).bind(node.host_id).bind(node.server_id).bind(credential).bind(node.display).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO management_audit(admin_id,action,object_path,request_diff,result,occurred_at) VALUES($1,'nexus.nodes.import','/api/plugins/sing-box/nexus-nodes',$2,$3,$4)")
            .bind(actor).bind(json!({"source_id":snapshot.source_id,"digest":digest})).bind(json!({"node_count":snapshot.nodes.len(),"state":"pending_agent"})).bind(now_timestamp()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(report))
}

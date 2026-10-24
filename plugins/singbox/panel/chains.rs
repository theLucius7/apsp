use crate::{
    AppState,
    auth::require_admin,
    error::{ApiError, ApiResult},
};
use apsp_compiler::Relay;
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchRequest {
    pub request_id: Uuid,
    pub items: Vec<BatchItem>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BatchItem {
    pub name: String,
    pub entry: BatchEntry,
    pub hops: Vec<BatchHop>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum BatchEntry {
    New {
        server_id: i64,
        public_host: String,
        sni: String,
        port: Option<i64>,
    },
    Existing {
        node_id: i64,
    },
}

#[derive(Clone, Deserialize, Serialize)]
pub struct BatchHop {
    pub kind: String,
    #[serde(default)]
    pub node_id: Option<i64>,
    #[serde(flatten)]
    pub additional: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BatchReceipt {
    pub request_id: Uuid,
    pub chain_ids: Vec<i64>,
    pub entry_node_ids: Vec<i64>,
}

pub async fn create_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BatchRequest>,
) -> ApiResult<(StatusCode, Json<BatchReceipt>)> {
    super::ordered_paths::create_batch(
        State(state),
        headers,
        super::subscription_sources::models::ProtectedJson(request),
    )
    .await
}

#[derive(Serialize, FromRow)]
pub struct Chain {
    pub id: i64,
    pub name: String,
    pub entry_node_id: i64,
    pub exit_node_id: i64,
    pub available: bool,
}
const SELECT: &str = "SELECT c.id,c.name,c.entry_node_id,c.exit_node_id,(n.enabled AND e.enabled AND n.deleted_at IS NULL AND e.deleted_at IS NULL AND n.protocol='vless-reality' AND e.protocol='vless-reality' AND ns.deleted_at IS NULL AND es.deleted_at IS NULL) AS available FROM singbox_live_chains c JOIN nodes n ON n.id=c.entry_node_id JOIN nodes e ON e.id=c.exit_node_id JOIN servers ns ON ns.id=n.server_id JOIN servers es ON es.id=e.server_id WHERE c.path_kind='legacy'";

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<Chain>>> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        sqlx::query_as(&format!("{SELECT} ORDER BY c.id"))
            .fetch_all(&state.pool)
            .await?,
    ))
}

/// Two-hop chains are no longer created; new chains are ordered (ADR 0079
/// phase 3, step S1d). Existing two-hop chains stay listed and removable.
pub async fn create(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<StatusCode> {
    require_admin(&state, &headers).await?;
    Err(ApiError::Conflict(
        "不再新建两跳链路；请使用“创建链路”创建有序链路".into(),
    ))
}

pub async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    super::entitlements::lock(&mut tx).await?;
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT path_kind FROM singbox_chains WHERE id=$1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let kind = kind.ok_or(ApiError::NotFound)?;
    if kind != "legacy" {
        return Err(ApiError::Conflict(
            "请通过对应链路资源接口删除完整路径".into(),
        ));
    }
    let servers: Vec<i64> = sqlx::query_scalar("SELECT unnest(ARRAY[n.server_id,e.server_id]) FROM singbox_live_chains c JOIN nodes n ON n.id=c.entry_node_id JOIN nodes e ON e.id=c.exit_node_id WHERE c.id=$1")
        .bind(id).fetch_all(&mut *tx).await?;
    super::proxy_resources::lock_cleanup_servers(&mut tx, &servers).await?;
    super::proxy_resources::ensure_chain_unreferenced(&mut tx, id).await?;
    super::business::mark_dirty(&mut tx, &servers).await?;
    if sqlx::query("UPDATE singbox_chains SET deleted_at=FLOOR(EXTRACT(EPOCH FROM clock_timestamp()))::bigint WHERE id=$1 AND deleted_at IS NULL AND path_kind='legacy'")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 0
    {
        return Err(ApiError::NotFound);
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn ensure_direct(tx: &mut Transaction<'_, Postgres>, node: i64) -> ApiResult<()> {
    let preserved: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_nexus_runtime_nodes WHERE node_id=$1)",
    )
    .bind(node)
    .fetch_one(&mut **tx)
    .await?;
    if preserved {
        return Err(ApiError::Conflict(
            "此迁移节点沿用原权限组，请修改套餐与原权限组映射".into(),
        ));
    }
    let entry: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_chains WHERE entry_node_id=$1 AND (deleted_at IS NULL OR (path_kind='ordered' AND phase<>'retired')))",
    )
    .bind(node)
    .fetch_one(&mut **tx)
    .await?;
    if entry {
        return Err(ApiError::Conflict(
            "此节点是链路入口，请通过包含该链路的策略组授权".into(),
        ));
    }
    Ok(())
}

#[derive(FromRow)]
struct RelayRow {
    exit_settings: serde_json::Value,
    chain_id: i64,
    entry_node_id: i64,
    exit_node_id: i64,
    uuid: Uuid,
    entry_protocol: String,
    exit_protocol: String,
    public_host: String,
    port: i32,
    sni: String,
    public_key: String,
    short_id: String,
}

pub(crate) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    at: i64,
) -> anyhow::Result<Vec<Relay>> {
    let rows = sqlx::query_as::<_, RelayRow>("SELECT c.id AS chain_id,c.entry_node_id,c.exit_node_id,c.relay_uuid AS uuid,n.protocol AS entry_protocol,e.protocol AS exit_protocol,e.public_host,e.port,e.sni,e.public_key,e.short_id,e.settings AS exit_settings FROM singbox_live_chains c JOIN nodes n ON n.id=c.entry_node_id JOIN nodes e ON e.id=c.exit_node_id JOIN servers ns ON ns.id=n.server_id JOIN servers es ON es.id=e.server_id WHERE (n.server_id=$1 OR e.server_id=$1) AND c.path_kind='legacy' AND n.enabled AND e.enabled AND n.deleted_at IS NULL AND e.deleted_at IS NULL AND n.protocol='vless-reality' AND e.protocol='vless-reality' AND ns.deleted_at IS NULL AND es.deleted_at IS NULL AND EXISTS(SELECT 1 FROM singbox_eligible_accesses($2) a WHERE a.node_id=n.id) ORDER BY c.id")
        .bind(server).bind(at).fetch_all(&mut **tx).await?;
    rows.into_iter()
        .map(|r| {
            let settings: apsp_compiler::NodeSettings = serde_json::from_value(r.exit_settings)?;
            anyhow::ensure!(
                r.entry_protocol == "vless-reality" && r.exit_protocol == "vless-reality",
                "two-hop relays require VLESS + Reality at both ends"
            );
            Ok(Relay {
                fingerprint: settings.reality.fingerprint,
                settings: settings.clone(),
                chain_id: r.chain_id,
                entry_node_id: r.entry_node_id,
                exit_node_id: r.exit_node_id,
                uuid: r.uuid,
                public_host: r.public_host,
                port: settings.public_port.unwrap_or(r.port.try_into()?),
                sni: r.sni,
                public_key: r.public_key,
                short_id: r.short_id,
            })
        })
        .collect()
}

/// The rich resource history a legacy chain received when it was created; only
/// test fixtures create legacy chains now.
#[cfg(test)]
async fn seed_legacy_projection_on(tx: &mut Transaction<'_, Postgres>, id: i64) -> ApiResult<()> {
    use super::ordered_paths::{
        models::{Capabilities, FrozenHop, FrozenVersion},
        storage,
    };
    let (entry, exit, relay): (i64,i64,Uuid) = sqlx::query_as("SELECT entry_node_id,exit_node_id,relay_uuid FROM singbox_chains WHERE id=$1 AND path_kind='legacy'")
        .bind(id).fetch_one(&mut **tx).await?;
    let mut endpoints = Vec::new();
    for node_id in [entry, exit] {
        let row: super::business::NodeRow = sqlx::query_as(&format!(
            "SELECT {} FROM nodes n WHERE n.id=$1",
            super::business::NODE_COLUMNS
        ))
        .bind(node_id)
        .fetch_one(&mut **tx)
        .await?;
        // A disabled legacy endpoint remains visible and unavailable as before.
        let node = row.model(vec![])?;
        let hash = storage::sha(&node)?;
        sqlx::query("INSERT INTO singbox_managed_endpoint_versions(id,node_id,server_id,snapshot,semantic_sha256,created_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(node_id,semantic_sha256) DO NOTHING")
            .bind(Uuid::new_v4()).bind(node_id).bind(row.server_id).bind(serde_json::to_value(&node).map_err(anyhow::Error::from)?).bind(&hash).bind(apsp_protocol::now_timestamp()).execute(&mut **tx).await?;
        let version_id = sqlx::query_scalar("SELECT id FROM singbox_managed_endpoint_versions WHERE node_id=$1 AND semantic_sha256=$2")
            .bind(node_id).bind(hash).fetch_one(&mut **tx).await?;
        endpoints.push(apsp_compiler::ManagedEndpointSnapshot {
            version_id,
            server_id: row.server_id,
            node,
        });
    }
    let exit = endpoints.pop().expect("legacy exit snapshot");
    let frozen = FrozenVersion {
        entry: endpoints.pop().expect("legacy entry snapshot"),
        hops: vec![FrozenHop::Managed {
            endpoint: Box::new(exit),
            relay_uuid: relay,
        }],
        legacy_relay_uuid: Some(relay),
    };
    storage::save_version(
        tx,
        id,
        1,
        &frozen,
        true,
        &Capabilities {
            tcp: true,
            udp: true,
        },
    )
    .await?;
    sqlx::query(
        "UPDATE singbox_chains SET applied_generation=1 WHERE id=$1 AND path_kind='legacy'",
    )
    .bind(id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
#[path = "chains/merge_tests.rs"]
mod merge_tests;

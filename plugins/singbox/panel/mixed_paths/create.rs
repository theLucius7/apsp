use super::models::*;
use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
use apsp_compiler::paths::{Hop, Path};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use sqlx::{Postgres, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

pub async fn batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<BatchRequest>,
) -> ApiResult<(StatusCode, Json<Receipt>)> {
    auth::require_admin(&state, &headers).await?;
    if request.request_id.is_nil() || request.items.is_empty() || request.items.len() > 32 {
        return Err(ApiError::BadRequest(
            "每批需包含 1 至 32 条链路和有效的请求标识".into(),
        ));
    }
    for item in &mut request.items {
        item.name = super::super::business::name(&item.name)?;
        if item.hops.is_empty() || item.hops.len() > 8 {
            return Err(ApiError::BadRequest(
                "每条链路需包含 1 至 8 个入口后的节点".into(),
            ));
        }
    }
    let hash = auth::hash_token(&serde_json::to_string(&request).map_err(anyhow::Error::from)?);
    let mut tx = state.pool.begin().await?;
    super::super::entitlements::lock(&mut tx).await?;
    if let Some((saved_hash, receipt)) = sqlx::query_as::<_, (String, serde_json::Value)>(
        "SELECT request_hash,receipt FROM singbox_chain_creation_requests WHERE request_id=$1",
    )
    .bind(request.request_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        if saved_hash != hash {
            return Err(ApiError::Conflict(
                "相同请求标识已用于不同内容，请重新提交".into(),
            ));
        }
        return Ok((
            StatusCode::OK,
            Json(serde_json::from_value(receipt).map_err(anyhow::Error::from)?),
        ));
    }
    let mut source_ids = BTreeSet::new();
    let mut server_ids = BTreeSet::new();
    for item in &request.items {
        match &item.entry {
            EntryInput::New { server_id, .. } => {
                server_ids.insert(*server_id);
            }
            EntryInput::Existing { node_id } => {
                server_ids.insert(node_on(&mut tx, *node_id).await?.server_id);
            }
        }
        for hop in &item.hops {
            match hop {
                HopInput::Managed { node_id } => {
                    server_ids.insert(node_on(&mut tx, *node_id).await?.server_id);
                }
                HopInput::Subscription { source_id, .. } => {
                    source_ids.insert(*source_id);
                }
            }
        }
    }
    // After the source migration new chains are ordered (ADR 0079 phase 3, S1d);
    // a replay of an earlier batch above still returns its receipt.
    if super::super::source_migration::migrated(&mut tx).await? {
        return Err(ApiError::Conflict(
            "订阅来源已迁移，不再新建混合链路；请使用“创建链路”创建有序链路".into(),
        ));
    }
    sqlx::query(
        "SELECT id FROM singbox_subscription_sources WHERE id=ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(source_ids.into_iter().collect::<Vec<_>>())
    .fetch_all(&mut *tx)
    .await?;
    for id in &server_ids {
        super::super::business::lock_server(&mut tx, *id).await?;
        super::super::settings::require_enabled(&mut tx, *id).await?;
    }
    let mut receipt = Receipt {
        request_id: request.request_id,
        chain_ids: Vec::new(),
        entry_node_ids: Vec::new(),
    };
    let mut previews = Vec::new();
    for item in &request.items {
        let entry = entry_on(&mut tx, item).await?;
        let conflict:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_chains WHERE (deleted_at IS NULL OR (path_kind='ordered' AND phase<>'retired')) AND (entry_node_id=$1 OR exit_node_id=$1)) OR EXISTS(SELECT 1 FROM singbox_ordered_chain_hops h JOIN singbox_chains c ON c.id=h.chain_id WHERE c.path_kind='ordered' AND (c.deleted_at IS NULL OR c.phase<>'retired') AND h.managed_node_id=$1 AND (h.generation=ANY(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) OR EXISTS(SELECT 1 FROM unnest(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) AS selected(generation) WHERE selected.generation IS NOT NULL AND NOT EXISTS(SELECT 1 FROM singbox_ordered_chain_versions v WHERE v.chain_id=c.id AND v.generation=selected.generation)))) OR EXISTS(SELECT 1 FROM accesses WHERE node_id=$1) OR EXISTS(SELECT 1 FROM singbox_policy_nodes WHERE node_id=$1) OR EXISTS(SELECT 1 FROM singbox_chain_hops h JOIN singbox_live_chains c ON c.id=h.chain_id WHERE h.managed_node_id=$1)")
            .bind(entry.id).fetch_one(&mut *tx).await?;
        if conflict || receipt.entry_node_ids.contains(&entry.id) {
            return Err(ApiError::Conflict(
                "链路入口需要使用未授权、未被其他路径引用的独立节点".into(),
            ));
        }
        if !entry.enabled || entry.protocol != "vless-reality" {
            return Err(ApiError::BadRequest(
                "链路入口必须是已启用的 Reality 节点".into(),
            ));
        }
        let port_conflict:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE server_id=$1 AND deleted_at IS NULL AND port=18086)").bind(entry.server_id).fetch_one(&mut *tx).await?;
        if port_conflict {
            return Err(ApiError::Conflict(
                "入口服务器的 18086 端口已分配给节点，无法启用路径验证".into(),
            ));
        }
        let id:i64=sqlx::query_scalar("INSERT INTO singbox_chains(name,entry_node_id,path_kind) VALUES($1,$2,'mixed') RETURNING id").bind(&item.name).bind(entry.id).fetch_one(&mut *tx).await?;
        let path = build_path_on(&mut tx, id, 1, entry.server_id, entry.id, &item.hops).await?;
        insert_version_on(&mut tx, &path, &item.hops, None).await?;
        let url = format!("{}/healthz", state.config.public_url.trim_end_matches('/'));
        if !url.starts_with("https://") {
            return Err(ApiError::Conflict(
                "混合链路需要面板配置可达的 HTTPS 地址，用于经过指定路径验证连接".into(),
            ));
        }
        sqlx::query("INSERT INTO singbox_path_controls(server_id,secret,test_url) VALUES($1,$2,$3) ON CONFLICT(server_id) DO NOTHING").bind(entry.server_id).bind(auth::random_token()).bind(url).execute(&mut *tx).await?;
        previews.push(path);
        receipt.chain_ids.push(id);
        receipt.entry_node_ids.push(entry.id);
    }
    validate_frozen_on(&mut tx, &previews).await?;
    sqlx::query("INSERT INTO singbox_chain_creation_requests(request_id,request_hash,receipt,created_at) VALUES($1,$2,$3,$4)")
        .bind(request.request_id).bind(hash).bind(serde_json::to_value(&receipt).map_err(anyhow::Error::from)?).bind(apsp_protocol::now_timestamp()).execute(&mut *tx).await?;
    super::super::business::mark_dirty(&mut tx, &server_ids.into_iter().collect::<Vec<_>>())
        .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(receipt)))
}

pub(crate) async fn build_path_on(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    generation: u64,
    entry_server: i64,
    entry_node: i64,
    inputs: &[HopInput],
) -> ApiResult<Path> {
    let mut hops = Vec::new();
    for input in inputs {
        match input {
            HopInput::Managed { node_id } => {
                let node = node_on(tx, *node_id).await?;
                super::super::settings::require_enabled(tx, node.server_id).await?;
                let used: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM singbox_chains WHERE entry_node_id=$1 AND (deleted_at IS NULL OR (path_kind='ordered' AND phase<>'retired')))",
                )
                .bind(node_id)
                .fetch_one(&mut **tx)
                .await?;
                if used {
                    return Err(ApiError::Conflict(
                        "链路内部节点不能是其他链路的专用入口".into(),
                    ));
                }
                hops.push(Hop::Managed {
                    server_id: node.server_id,
                    endpoint: Box::new(node.model(Vec::new())?),
                    identity: Uuid::new_v4(),
                });
            }
            HopInput::Subscription {
                source_id,
                external_node_id,
                node_version_id,
                ..
            } => {
                let version = super::super::sources::load_version_on(
                    tx,
                    *source_id,
                    *external_node_id,
                    *node_version_id,
                    true,
                )
                .await?;
                hops.push(Hop::External {
                    node_id: *external_node_id,
                    version_id: *node_version_id,
                    outbound: version.outbound,
                });
            }
        }
    }
    let path = Path {
        chain_id: id,
        generation,
        entry_server_id: entry_server,
        entry_node_id: entry_node,
        active: false,
        hops,
    };
    apsp_compiler::paths::validate(&path).map_err(path_error)?;
    Ok(path)
}

async fn entry_on(
    tx: &mut Transaction<'_, Postgres>,
    input: &ChainInput,
) -> ApiResult<super::super::business::NodeRow> {
    match &input.entry {
        EntryInput::Existing { node_id } => node_on(tx, *node_id).await,
        EntryInput::New {
            server_id,
            public_host,
            sni,
            port,
        } => {
            let port=match port {
                Some(value) if *value!=0 && ![18085,18086].contains(value)=>i32::from(*value),
                Some(_)=>return Err(ApiError::BadRequest("监听端口无效或占用了本机保留端口".into())),
                None=>sqlx::query_scalar::<_,i32>("SELECT p FROM generate_series(20000,29999) p WHERE NOT EXISTS(SELECT 1 FROM nodes WHERE server_id=$1 AND deleted_at IS NULL AND port=p) ORDER BY p LIMIT 1").bind(server_id).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::Conflict("没有可分配的入口端口".into()))?,
            };
            let used:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE server_id=$1 AND port=$2 AND deleted_at IS NULL)").bind(server_id).bind(port).fetch_one(&mut **tx).await?;
            if used {
                return Err(ApiError::Conflict(
                    "本批链路的入口端口已被占用，请修改后重试".into(),
                ));
            }
            let (private, public) = super::super::business::generate_reality_keypair();
            let query = format!(
                "INSERT INTO nodes AS n (name,server_id,protocol,port,public_host,sni,private_key,public_key,short_id,protocol_config) VALUES($1,$2,'vless-reality',$3,$4,$5,$6,$7,$8,$9) RETURNING {}",
                super::super::business::NODE_COLUMNS
            );
            let row: super::super::business::NodeRow = sqlx::query_as(&query)
                .bind(&input.name)
                .bind(server_id)
                .bind(port)
                .bind(public_host)
                .bind(sni)
                .bind(private)
                .bind(public)
                .bind(super::super::business::short_id())
                .bind(
                    serde_json::to_value(apsp_compiler::ProtocolConfig::default())
                        .map_err(anyhow::Error::from)?,
                )
                .fetch_one(&mut **tx)
                .await?;
            super::super::business::validate_node(&row)?;
            super::super::nodes::validate_server_config(tx, &row).await?;
            Ok(row)
        }
    }
}

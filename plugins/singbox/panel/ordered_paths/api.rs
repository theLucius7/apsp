use super::super::{
    business,
    chains::{BatchEntry, BatchReceipt, BatchRequest},
    subscription_sources::models::ProtectedJson,
};
use super::{models::*, storage};
use crate::{
    AppState,
    auth::require_admin,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum HopInput {
    Managed {
        node_id: i64,
    },
    Subscription {
        source_id: i64,
        external_node_id: Uuid,
        node_version_id: Uuid,
        update_mode: String,
    },
}
fn parsed_hops(item: &super::super::chains::BatchItem, index: usize) -> ApiResult<Vec<HopInput>> {
    if item.hops.is_empty() || item.hops.len() > 8 {
        return Err(ApiError::BadRequest(format!(
            "第 {} 条：入口后需要 1 至 8 个有序代理跳",
            index + 1
        )));
    }
    item.hops
        .iter()
        .enumerate()
        .map(|(position, hop)| {
            let mut value = json!({"kind":hop.kind});
            if let Some(node_id) = hop.node_id {
                value["node_id"] = json!(node_id);
            }
            for (key, field) in &hop.additional {
                value[key] = field.clone();
            }
            let parsed: HopInput = serde_json::from_value(value).map_err(|_| {
                ApiError::BadRequest(format!(
                    "第 {} 条第 {} 跳：请选择受管节点或有效的订阅节点版本",
                    index + 1,
                    position + 1
                ))
            })?;
            match &parsed {
                HopInput::Managed { node_id } if *node_id <= 0 => {
                    return Err(ApiError::BadRequest(format!(
                        "第 {} 条第 {} 跳：节点编号必须为正整数",
                        index + 1,
                        position + 1
                    )));
                }
                HopInput::Subscription {
                    source_id,
                    external_node_id,
                    node_version_id,
                    update_mode,
                } if *source_id <= 0
                    || external_node_id.is_nil()
                    || node_version_id.is_nil()
                    || !matches!(update_mode.as_str(), "follow_node" | "pinned") =>
                {
                    return Err(ApiError::BadRequest(format!(
                        "第 {} 条第 {} 跳：来源、节点、版本或更新方式无效",
                        index + 1,
                        position + 1
                    )));
                }
                _ => {}
            }
            Ok(parsed)
        })
        .collect()
}
fn item_error(index: usize, position: Option<usize>, error: ApiError) -> ApiError {
    let prefix = position
        .map(|position| format!("第 {} 条第 {} 跳", index + 1, position + 1))
        .unwrap_or_else(|| format!("第 {} 条", index + 1));
    match error {
        ApiError::NotFound => ApiError::BadRequest(format!("{prefix}：节点不存在或已删除")),
        ApiError::BadRequest(reason) => ApiError::BadRequest(format!("{prefix}：{reason}")),
        ApiError::Conflict(reason) => ApiError::Conflict(format!("{prefix}：{reason}")),
        other => other,
    }
}
pub async fn create_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    ProtectedJson(mut request): ProtectedJson<BatchRequest>,
) -> ApiResult<(StatusCode, Json<BatchReceipt>)> {
    require_admin(&state, &headers).await?;
    if request.request_id.is_nil() || request.items.is_empty() || request.items.len() > 32 {
        return Err(ApiError::BadRequest(
            "一次创建需要有效请求 ID 和 1 至 32 条链路".into(),
        ));
    }
    let mut legacy_items = request.items.clone();
    for item in &mut legacy_items {
        item.name = business::name(&item.name)?;
    }
    let legacy_hash = storage::sha(&legacy_items)?;
    let mut hops = Vec::new();
    for (index, item) in request.items.iter_mut().enumerate() {
        item.name = business::name(&item.name).map_err(|error| item_error(index, None, error))?;
        match &mut item.entry {
            BatchEntry::New {
                server_id,
                public_host,
                sni,
                port,
            } => {
                if *server_id <= 0 {
                    return Err(ApiError::BadRequest(format!(
                        "第 {} 条：请选择有效入口服务器",
                        index + 1
                    )));
                }
                *public_host = public_host.trim().into();
                *sni = sni.trim().into();
                if let Some(port) = port {
                    super::super::nodes::validate_port(*port)
                        .map_err(|error| item_error(index, None, error))?;
                }
            }
            BatchEntry::Existing { node_id } if *node_id <= 0 => {
                return Err(ApiError::BadRequest(format!(
                    "第 {} 条：请选择有效入口节点",
                    index + 1
                )));
            }
            _ => {}
        }
        hops.push(parsed_hops(item, index)?);
    }
    let hash = storage::sha(&request.items)?;
    let mut tx = state.pool.begin().await?;
    super::super::entitlements::lock(&mut tx).await?;
    let existing: Option<(String, Value)> = sqlx::query_as(
        "SELECT request_sha256,receipt FROM singbox_ordered_chain_creation_requests WHERE request_id=$1",
    )
    .bind(request.request_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((previous, receipt)) = existing {
        if hash != previous && legacy_hash != previous {
            return Err(ApiError::Conflict(
                "此请求 ID 已用于不同内容，请为修改后的草稿使用新的请求 ID".into(),
            ));
        }
        let receipt = serde_json::from_value(receipt).map_err(anyhow::Error::from)?;
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(receipt)));
    }
    let mut source_ids = BTreeSet::new();
    let mut servers = BTreeSet::new();
    for (index, (item, hops)) in request.items.iter().zip(&hops).enumerate() {
        let mut node_ids = Vec::new();
        match &item.entry {
            BatchEntry::New { server_id, .. } => {
                servers.insert(*server_id);
            }
            BatchEntry::Existing { node_id } => node_ids.push(*node_id),
        }
        for hop in hops {
            match hop {
                HopInput::Managed { node_id } => node_ids.push(*node_id),
                HopInput::Subscription { source_id, .. } => {
                    source_ids.insert(*source_id);
                }
            }
        }
        for node in node_ids {
            let server = sqlx::query_scalar::<_, i64>(
                "SELECT server_id FROM nodes WHERE id=$1 AND deleted_at IS NULL",
            )
            .bind(node)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)
            .map_err(|error| item_error(index, None, error))?;
            servers.insert(server);
        }
    }
    for source in source_ids {
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_ordered_subscription_sources WHERE id=$1 AND deleted_at IS NULL)").bind(source).fetch_one(&mut *tx).await?;
        if !exists {
            return Err(ApiError::BadRequest(format!(
                "订阅来源 #{source} 不存在或已删除"
            )));
        }
        sqlx::query("SELECT id FROM singbox_ordered_subscription_sources WHERE id=$1 FOR UPDATE")
            .bind(source)
            .fetch_one(&mut *tx)
            .await?;
    }
    for server in &servers {
        let live:bool=sqlx::query_scalar("SELECT deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM server_retirements r WHERE r.server_id=s.id AND r.status IN ('pending','failed','offline_unconfirmed')) FROM servers s WHERE id=$1 FOR UPDATE").bind(server).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::BadRequest(format!("服务器 #{server} 不存在")))?;
        if !live {
            return Err(ApiError::Conflict(format!(
                "服务器 #{server} 已退役或正在退役，不能创建链路"
            )));
        }
        super::super::settings::require_enabled(&mut tx, *server).await?;
    }
    storage::probe_target(&state)?;
    storage::require_capabilities(&mut tx, &servers).await?;
    let mut receipt = BatchReceipt {
        request_id: request.request_id,
        chain_ids: Vec::new(),
        entry_node_ids: Vec::new(),
    };
    let all_hop_ids: BTreeSet<i64> = hops
        .iter()
        .flatten()
        .filter_map(|hop| match hop {
            HopInput::Managed { node_id } => Some(*node_id),
            _ => None,
        })
        .collect();
    for (index, (item, hops)) in request.items.into_iter().zip(hops).enumerate() {
        let entry = match item.entry {
            BatchEntry::Existing { node_id } => node_id,
            BatchEntry::New {
                server_id,
                public_host,
                sni,
                port,
            } => {
                super::super::nodes::create_locked(
                    &mut tx,
                    super::super::nodes::CreateNode {
                        enabled: Some(true),
                        settings: Default::default(),
                        name: item.name.clone(),
                        server_id,
                        public_host,
                        sni,
                        protocol_config: Default::default(),
                        port,
                    },
                )
                .await
                .map_err(|error| item_error(index, None, error))?
                .id
            }
        };
        if hops
            .iter()
            .any(|hop| matches!(hop,HopInput::Managed{node_id}if *node_id==entry))
        {
            return Err(ApiError::BadRequest(format!(
                "第 {} 条：路径不能回到自身入口节点",
                index + 1
            )));
        }
        if all_hop_ids.contains(&entry) {
            return Err(ApiError::Conflict(format!(
                "第 {} 条：专用入口不能同时作为任一路径的内部跳",
                index + 1
            )));
        }
        let conflict:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_chains WHERE (entry_node_id=$1 OR exit_node_id=$1) AND (deleted_at IS NULL OR (path_kind='ordered' AND phase<>'retired'))) OR EXISTS(SELECT 1 FROM singbox_chain_hops h JOIN singbox_live_chains c ON c.id=h.chain_id WHERE h.managed_node_id=$1) OR EXISTS(SELECT 1 FROM singbox_ordered_chain_hops h JOIN singbox_chains c ON c.id=h.chain_id WHERE h.managed_node_id=$1 AND (c.deleted_at IS NULL OR c.phase<>'retired') AND (h.generation=ANY(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) OR EXISTS(SELECT 1 FROM unnest(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) AS selected(generation) WHERE selected.generation IS NOT NULL AND NOT EXISTS(SELECT 1 FROM singbox_ordered_chain_versions v WHERE v.chain_id=c.id AND v.generation=selected.generation)))) OR EXISTS(SELECT 1 FROM accesses WHERE node_id=$1) OR EXISTS(SELECT 1 FROM singbox_policy_nodes WHERE node_id=$1)").bind(entry).fetch_one(&mut *tx).await?;
        if conflict {
            return Err(ApiError::Conflict(format!(
                "第 {} 条：入口需要尚未授权且未被其他路径引用的独立节点",
                index + 1
            )));
        }
        let id = {
            let entry_snapshot = storage::freeze_endpoint(&mut tx, entry)
                .await
                .map_err(|error| item_error(index, None, error))?;
            let mut frozen = FrozenVersion {
                entry: entry_snapshot,
                hops: Vec::new(),
                legacy_relay_uuid: None,
            };
            let mut endpoint_servers = BTreeSet::from([frozen.entry.server_id]);
            let mut identities = BTreeSet::new();
            for (position, hop) in hops.into_iter().enumerate() {
                let hop = match hop {
                    HopInput::Managed { node_id } => {
                        let nested:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_chains WHERE entry_node_id=$1 AND (deleted_at IS NULL OR (path_kind='ordered' AND phase<>'retired')))").bind(node_id).fetch_one(&mut *tx).await?;
                        if nested {
                            return Err(ApiError::Conflict(format!(
                                "第 {} 条第 {} 跳：不能引用其他链路的专用入口",
                                index + 1,
                                position + 1
                            )));
                        }
                        let endpoint = storage::freeze_endpoint(&mut tx, node_id)
                            .await
                            .map_err(|error| item_error(index, Some(position), error))?;
                        if !endpoint_servers.insert(endpoint.server_id) {
                            return Err(ApiError::BadRequest(format!(
                                "第 {} 条第 {} 跳：受管跳不能回到入口服务器或重复同一服务器",
                                index + 1,
                                position + 1
                            )));
                        }
                        FrozenHop::Managed {
                            endpoint: Box::new(endpoint),
                            relay_uuid: Uuid::new_v4(),
                        }
                    }
                    HopInput::Subscription {
                        source_id,
                        external_node_id,
                        node_version_id,
                        update_mode,
                    } => {
                        if !identities.insert((source_id, external_node_id)) {
                            return Err(ApiError::BadRequest(format!(
                                "第 {} 条第 {} 跳：同一订阅节点不能重复出现在路径中",
                                index + 1,
                                position + 1
                            )));
                        }
                        storage::external_hop(
                            &mut tx,
                            source_id,
                            external_node_id,
                            node_version_id,
                            &update_mode,
                        )
                        .await
                        .map_err(|error| item_error(index, Some(position), error))?
                    }
                };
                frozen.hops.push(hop);
            }
            let id:i64=sqlx::query_scalar("INSERT INTO singbox_chains(name,entry_node_id,path_kind,candidate_generation,phase,route_enabled) VALUES($1,$2,'ordered',1,'preparing_dependencies',FALSE) RETURNING id").bind(&item.name).bind(entry).fetch_one(&mut *tx).await?;
            let path = storage::compiler_path(id, 1, &frozen, false);
            apsp_compiler::validate_path(&path).map_err(|error| {
                ApiError::BadRequest(format!(
                    "第 {} 条：路径结构或下层承载不兼容：{error}",
                    index + 1
                ))
            })?;
            let caps = apsp_compiler::path_capabilities(&path).map_err(|error| {
                ApiError::BadRequest(format!("第 {} 条：路径承载无法确认：{error}", index + 1))
            })?;
            if !caps.tcp {
                return Err(ApiError::BadRequest(format!(
                    "第 {} 条：当前指定出站 HTTPS 探测需要路径支持 TCP，无法确认仅 UDP 路径",
                    index + 1
                )));
            }
            storage::save_version(
                &mut tx,
                id,
                1,
                &frozen,
                false,
                &Capabilities {
                    tcp: caps.tcp,
                    udp: caps.udp,
                },
            )
            .await?;
            storage::pin_runtimes(&state, &mut tx, id, 1, &frozen).await?;
            storage::reserve_probe_capacity(&state, &mut tx, frozen.entry.server_id).await?;
            super::publication::validate_candidate(&mut tx, &path, &frozen)
                .await
                .map_err(|error| item_error(index, None, error))?;
            id
        };
        receipt.chain_ids.push(id);
        receipt.entry_node_ids.push(entry);
    }
    business::mark_dirty(&mut tx, &servers.into_iter().collect::<Vec<_>>()).await?;
    sqlx::query("INSERT INTO singbox_ordered_chain_creation_requests(request_id,request_sha256,receipt,created_at) VALUES($1,$2,$3,$4)").bind(receipt.request_id).bind(hash).bind(json!(receipt)).bind(now_timestamp()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(receipt)))
}

async fn mutation_receipt(
    connection: &mut PgConnection,
    id: Uuid,
    hash: &str,
) -> ApiResult<Option<MutationReceipt>> {
    if id.is_nil() {
        return Err(ApiError::BadRequest("request_id 必须为有效 UUID".into()));
    }
    let previous: Option<(String, Value)> = sqlx::query_as(
        "SELECT request_sha256,receipt FROM singbox_resource_mutation_requests WHERE request_id=$1",
    )
    .bind(id)
    .fetch_optional(connection)
    .await?;
    match previous {
        Some((previous, _)) if previous != hash => {
            Err(ApiError::Conflict("此 request_id 已用于不同操作".into()))
        }
        Some((_, value)) => Ok(Some(
            serde_json::from_value(value).map_err(anyhow::Error::from)?,
        )),
        None => Ok(None),
    }
}
async fn save_receipt(
    connection: &mut PgConnection,
    hash: &str,
    receipt: &MutationReceipt,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO singbox_resource_mutation_requests(request_id,request_sha256,receipt,created_at) VALUES($1,$2,$3,$4)").bind(receipt.request_id).bind(hash).bind(json!(receipt)).bind(now_timestamp()).execute(connection).await?;
    Ok(())
}

pub async fn update_resource(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, id)): Path<(String, i64)>,
    ProtectedJson(mut input): ProtectedJson<ResourceUpdate>,
) -> ApiResult<Json<MutationReceipt>> {
    require_admin(&state, &headers).await?;
    if !matches!(kind.as_str(), "direct" | "chain") || id <= 0 || input.settings_revision <= 0 {
        return Err(ApiError::BadRequest("请选择有效资源及设置版本".into()));
    }
    if let Some(name) = &mut input.name {
        *name = business::name(name)?;
    }
    if let Some(name) = &mut input.entry_name {
        *name = business::name(name)?;
    }
    if input.name.is_none()
        && input.entry_name.is_none()
        && input.public_host.is_none()
        && input.port.is_none()
        && input.sni.is_none()
    {
        return Err(ApiError::BadRequest("至少提供一个可编辑字段".into()));
    }
    let hash = storage::sha(&json!({"operation":"update","kind":kind,"id":id,"input":input}))?;
    let mut tx = state.pool.begin().await?;
    super::super::entitlements::lock(&mut tx).await?;
    if let Some(receipt) = mutation_receipt(&mut tx, input.request_id, &hash).await? {
        tx.commit().await?;
        return Ok(Json(receipt));
    }
    let chain = if kind == "chain" {
        Some(storage::chain(&mut tx, id, true).await?)
    } else {
        None
    };
    if chain
        .as_ref()
        .is_some_and(|chain| chain.deleted_at.is_some())
    {
        return Err(ApiError::NotFound);
    }
    if chain
        .as_ref()
        .is_some_and(|chain| chain.settings_revision != input.settings_revision)
    {
        return Err(ApiError::Conflict("资源设置已被修改，请刷新后重试".into()));
    }
    let runtime_change = input.public_host.is_some() || input.port.is_some() || input.sni.is_some();
    if runtime_change
        && chain
            .as_ref()
            .is_some_and(|chain| chain.candidate_generation.is_some())
    {
        return Err(ApiError::Conflict(
            "链路仍在应用候选代数，请等待确认后修改运行参数".into(),
        ));
    }
    let entry_id = chain
        .as_ref()
        .map(|chain| chain.entry_node_id)
        .unwrap_or(id);
    if kind == "direct" {
        super::super::chains::ensure_direct(&mut tx, entry_id).await?;
    }
    let query = format!(
        "SELECT {} FROM nodes n WHERE n.id=$1 AND n.deleted_at IS NULL FOR UPDATE",
        business::NODE_COLUMNS
    );
    let mut node = sqlx::query_as::<_, business::NodeRow>(&query)
        .bind(entry_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let previous = node.clone();
    let node_revision: i64 = sqlx::query_scalar("SELECT resource_revision FROM nodes WHERE id=$1")
        .bind(entry_id)
        .fetch_one(&mut *tx)
        .await?;
    if kind == "direct" && node_revision != input.settings_revision {
        return Err(ApiError::Conflict("资源设置已被修改，请刷新后重试".into()));
    }
    business::lock_server(&mut tx, node.server_id).await?;
    if let Some(name) = input
        .entry_name
        .clone()
        .or_else(|| (kind == "direct").then(|| input.name.clone()).flatten())
    {
        node.name = name;
    }
    if let Some(host) = &input.public_host {
        node.public_host = host.trim().into();
    }
    if let Some(sni) = &input.sni {
        node.sni = sni.trim().into();
    }
    if let Some(port) = input.port {
        node.port = super::super::nodes::validate_port(port)?;
    }
    business::validate_node(&node)?;
    super::super::nodes::validate_server_config(&mut tx, &node).await?;
    storage::ensure_node_edit_safe(&mut tx, &node, &previous).await?;
    sqlx::query("UPDATE nodes SET name=$2,public_host=$3,sni=$4,port=$5,resource_revision=resource_revision+1 WHERE id=$1").bind(entry_id).bind(&node.name).bind(&node.public_host).bind(&node.sni).bind(node.port).execute(&mut *tx).await?;
    let (mut revision, mut generation) = (node_revision + 1, None);
    if let Some(chain) = chain {
        revision = chain
            .settings_revision
            .checked_add(1)
            .ok_or_else(|| ApiError::Conflict("资源版本已达到上限".into()))?;
        generation = Some(chain.desired_generation);
        sqlx::query(
            "UPDATE singbox_chains SET name=COALESCE($2,name),settings_revision=$3 WHERE id=$1",
        )
        .bind(id)
        .bind(&input.name)
        .bind(revision)
        .execute(&mut *tx)
        .await?;
        if runtime_change && chain.path_kind == "ordered" {
            generation =
                Some(super::lifecycle::candidate_for_endpoint_change(&state, &mut tx, id).await?);
        }
    }
    business::mark_dirty(&mut tx, &[node.server_id]).await?;
    let receipt = MutationReceipt {
        request_id: input.request_id,
        kind,
        id,
        settings_revision: revision,
        generation,
    };
    save_receipt(&mut tx, &hash, &receipt).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}
pub async fn apply_node_versions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    ProtectedJson(input): ProtectedJson<ApplyVersions>,
) -> ApiResult<Json<MutationReceipt>> {
    require_admin(&state, &headers).await?;
    if id <= 0
        || input.settings_revision <= 0
        || input.generation <= 0
        || input.versions.is_empty()
        || input.versions.len() > 8
    {
        return Err(ApiError::BadRequest(
            "请选择有效链路、代数和 1 至 8 个同节点版本".into(),
        ));
    }
    let hash = storage::sha(&json!({"operation":"apply_versions","id":id,"input":input}))?;
    let mut tx = state.pool.begin().await?;
    super::super::entitlements::lock(&mut tx).await?;
    if let Some(receipt) = mutation_receipt(&mut tx, input.request_id, &hash).await? {
        tx.commit().await?;
        return Ok(Json(receipt));
    }
    let chain = storage::chain(&mut tx, id, true).await?;
    if chain.deleted_at.is_some() {
        return Err(ApiError::NotFound);
    }
    if chain.settings_revision != input.settings_revision
        || chain.desired_generation != input.generation
    {
        return Err(ApiError::Conflict(
            "链路设置或代数已改变，请刷新后重试".into(),
        ));
    }
    if chain.path_kind != "ordered" || chain.candidate_generation.is_some() {
        return Err(ApiError::Conflict(
            "当前链路不能应用另一组节点版本，请等待候选完成".into(),
        ));
    }
    let mut frozen = storage::version(&mut tx, id, chain.desired_generation)
        .await?
        .snapshot;
    let mut positions = BTreeSet::new();
    for selection in &input.versions {
        if selection.hop_position == 0
            || selection.node_version_id.is_nil()
            || !positions.insert(selection.hop_position)
        {
            return Err(ApiError::BadRequest("跳位置必须有效且不能重复".into()));
        }
        let hop = frozen
            .hops
            .get_mut(selection.hop_position - 1)
            .ok_or_else(|| ApiError::BadRequest("路径中没有该跳".into()))?;
        let FrozenHop::Subscription {
            source_id,
            identity_epoch,
            external_node_id,
            update_mode,
            ..
        } = hop
        else {
            return Err(ApiError::BadRequest(
                "该跳不是订阅节点，不能使用来源版本更新".into(),
            ));
        };
        // Explicit application selects a valid historical version of this exact identity.
        // Preserve its configured follow/pinned mode without substituting a newer version.
        let mut replacement = storage::external_hop(
            &mut tx,
            *source_id,
            *external_node_id,
            selection.node_version_id,
            "pinned",
        )
        .await?;
        if let FrozenHop::Subscription {
            update_mode: mode, ..
        } = &mut replacement
        {
            *mode = update_mode.clone();
        }
        if let FrozenHop::Subscription {
            identity_epoch: new_epoch,
            ..
        } = &replacement
            && *new_epoch != *identity_epoch
        {
            return Err(ApiError::Conflict(
                "来源身份已更换，不能替换原节点身份".into(),
            ));
        }
        *hop = replacement;
    }
    let generation = super::lifecycle::start_candidate(&state, &mut tx, &chain, frozen).await?;
    let revision = chain.settings_revision + 1;
    sqlx::query("UPDATE singbox_chains SET settings_revision=$2 WHERE id=$1")
        .bind(id)
        .bind(revision)
        .execute(&mut *tx)
        .await?;
    let receipt = MutationReceipt {
        request_id: input.request_id,
        kind: "chain".into(),
        id,
        settings_revision: revision,
        generation: Some(generation),
    };
    save_receipt(&mut tx, &hash, &receipt).await?;
    tx.commit().await?;
    Ok(Json(receipt))
}

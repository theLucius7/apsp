use super::models::*;
use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use apsp_compiler::{ManagedEndpointSnapshot, OrderedPath};
use apsp_protocol::now_timestamp;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Postgres, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

type ExternalNodeFact = (
    i64,
    bool,
    Option<i64>,
    Option<Uuid>,
    i64,
    String,
    Option<Uuid>,
    Option<Uuid>,
    bool,
    Option<Value>,
    Option<String>,
    Uuid,
);

pub(crate) fn sha(value: &impl serde::Serialize) -> ApiResult<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(anyhow::Error::from)?)
    ))
}
pub(crate) async fn chain(
    connection: &mut PgConnection,
    id: i64,
    lock: bool,
) -> ApiResult<ChainRow> {
    sqlx::query_as(&format!(
        "SELECT {CHAIN_COLUMNS} FROM singbox_chains WHERE id=$1 AND path_kind IN ('legacy','ordered'){}",
        if lock { " FOR UPDATE" } else { "" }
    ))
    .bind(id)
    .fetch_optional(connection)
    .await?
    .ok_or(ApiError::NotFound)
}
/// The lifecycle also drives the candidate of a mixed chain under conversion;
/// that chain stays mixed until its entry switches.
pub(crate) async fn lifecycle_chain(
    connection: &mut PgConnection,
    id: i64,
    lock: bool,
) -> ApiResult<ChainRow> {
    sqlx::query_as(&format!(
        "SELECT {CHAIN_COLUMNS} FROM singbox_chains WHERE id=$1 AND (path_kind IN ('legacy','ordered') OR EXISTS(SELECT 1 FROM singbox_mixed_conversions m WHERE m.chain_id=singbox_chains.id AND m.state='preparing')){}",
        if lock { " FOR UPDATE" } else { "" }
    ))
    .bind(id)
    .fetch_optional(connection)
    .await?
    .ok_or(ApiError::NotFound)
}
pub(crate) async fn version(
    connection: &mut PgConnection,
    id: i64,
    generation: i64,
) -> ApiResult<VersionRow> {
    sqlx::query_as("SELECT generation,legacy,capabilities,snapshot FROM singbox_ordered_chain_versions WHERE chain_id=$1 AND generation=$2").bind(id).bind(generation).fetch_optional(connection).await?.ok_or(ApiError::NotFound)
}
pub(crate) fn compiler_path(
    chain_id: i64,
    generation: i64,
    frozen: &FrozenVersion,
    active: bool,
) -> OrderedPath {
    OrderedPath {
        chain_id,
        generation: generation as u64,
        entry_node_id: frozen.entry.node.id,
        entry_server_id: frozen.entry.server_id,
        hops: frozen.hops.iter().map(FrozenHop::compiler).collect(),
        active,
    }
}
pub(crate) async fn freeze_endpoint(
    tx: &mut Transaction<'_, Postgres>,
    node_id: i64,
) -> ApiResult<ManagedEndpointSnapshot> {
    let query = format!(
        "SELECT {} FROM nodes n WHERE id=$1 AND deleted_at IS NULL FOR UPDATE",
        super::super::business::NODE_COLUMNS
    );
    let row = sqlx::query_as::<_, super::super::business::NodeRow>(&query)
        .bind(node_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let node = row
        .model(vec![])
        .map_err(|_| ApiError::BadRequest(format!("节点 #{node_id} 的协议参数无法解析")))?;
    if !node.enabled || !node.protocol_config.is_reality() {
        return Err(ApiError::BadRequest(format!(
            "节点 #{node_id} 必须为已启用的 VLESS + Reality"
        )));
    }
    super::super::business::validate_node(&row)?;
    super::super::nodes::validate_server_config(tx, &row).await?;
    let hash = sha(&node)?;
    let proposed = Uuid::new_v4();
    sqlx::query("INSERT INTO singbox_managed_endpoint_versions(id,node_id,server_id,snapshot,semantic_sha256,created_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(node_id,semantic_sha256) DO NOTHING")
        .bind(proposed).bind(node_id).bind(row.server_id).bind(json!(node)).bind(&hash).bind(now_timestamp()).execute(&mut **tx).await?;
    let id = sqlx::query_scalar(
        "SELECT id FROM singbox_managed_endpoint_versions WHERE node_id=$1 AND semantic_sha256=$2",
    )
    .bind(node_id)
    .bind(hash)
    .fetch_one(&mut **tx)
    .await?;
    Ok(ManagedEndpointSnapshot {
        version_id: id,
        server_id: row.server_id,
        node,
    })
}
pub(crate) async fn external_hop(
    connection: &mut PgConnection,
    source_id: i64,
    node_id: Uuid,
    version_id: Uuid,
    mode: &str,
) -> ApiResult<FrozenHop> {
    let row:Option<ExternalNodeFact>=sqlx::query_as("SELECT s.identity_epoch,s.archived,s.deleted_at,s.current_success_revision,n.identity_epoch,n.identity_state,n.latest_version,n.last_seen_revision,v.supported,v.normalized_config,v.content_digest,v.source_revision_id FROM singbox_ordered_subscription_sources s JOIN singbox_ordered_external_nodes n ON n.source_id=s.id JOIN singbox_ordered_external_node_versions v ON v.node_id=n.id WHERE s.id=$1 AND n.id=$2 AND v.id=$3 FOR UPDATE OF s")
        .bind(source_id).bind(node_id).bind(version_id).fetch_optional(connection).await?;
    let Some((
        epoch,
        archived,
        deleted,
        current,
        node_epoch,
        identity,
        latest,
        seen,
        supported,
        config,
        digest,
        source_revision,
    )) = row
    else {
        return Err(ApiError::BadRequest(
            "订阅节点版本不属于所选来源或节点".into(),
        ));
    };
    if archived
        || deleted.is_some()
        || epoch != node_epoch
        || identity != "unique"
        || (mode == "follow_node" && latest != Some(version_id))
        || seen != current
        || current.is_none()
        || !supported
    {
        return Err(ApiError::Conflict(
            "订阅节点已缺失、来源已更换或归档，或节点身份/协议不可用，请刷新后重新选择".into(),
        ));
    }
    if !matches!(mode, "follow_node" | "pinned") {
        return Err(ApiError::BadRequest("请选择跟随节点或固定当前版本".into()));
    }
    let raw = config.ok_or_else(|| ApiError::BadRequest("订阅节点没有有效的规范化配置".into()))?;
    let outbound: apsp_compiler::external::NormalizedOutbound = serde_json::from_value(raw.clone())
        .map_err(|_| ApiError::BadRequest("订阅节点版本无法解析".into()))?;
    if serde_json::to_value(&outbound).map_err(anyhow::Error::from)? != raw {
        return Err(ApiError::BadRequest(
            "订阅节点版本包含未识别或丢失的必要参数".into(),
        ));
    }
    Ok(FrozenHop::Subscription {
        source_id,
        identity_epoch: epoch,
        external_node_id: node_id,
        node_version_id: version_id,
        source_revision_id: source_revision,
        update_mode: mode.into(),
        outbound: Box::new(outbound),
        content_digest: digest
            .ok_or_else(|| ApiError::BadRequest("订阅节点缺少完整版本身份".into()))?,
    })
}
pub(crate) async fn save_version(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    generation: i64,
    frozen: &FrozenVersion,
    legacy: bool,
    caps: &Capabilities,
) -> ApiResult<()> {
    let hash = sha(frozen)?;
    sqlx::query("INSERT INTO singbox_ordered_chain_versions(chain_id,generation,legacy,entry_endpoint_version,semantic_sha256,capabilities,snapshot,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(id).bind(generation).bind(legacy).bind(frozen.entry.version_id).bind(hash).bind(json!(caps)).bind(json!(frozen)).bind(now_timestamp()).execute(&mut **tx).await?;
    for (index, hop) in frozen.hops.iter().enumerate() {
        match hop {
            FrozenHop::Managed {
                endpoint,
                relay_uuid,
            } => {
                sqlx::query("INSERT INTO singbox_ordered_chain_hops(chain_id,generation,position,kind,endpoint_version_id,managed_node_id,managed_server_id,relay_uuid) VALUES($1,$2,$3,'managed',$4,$5,$6,$7)").bind(id).bind(generation).bind((index+1) as i32).bind(endpoint.version_id).bind(endpoint.node.id).bind(endpoint.server_id).bind(relay_uuid).execute(&mut **tx).await?;
            }
            FrozenHop::Subscription {
                source_id,
                identity_epoch,
                external_node_id,
                node_version_id,
                source_revision_id,
                update_mode,
                ..
            } => {
                sqlx::query("INSERT INTO singbox_ordered_chain_hops(chain_id,generation,position,kind,source_id,identity_epoch,external_node_id,node_version_id,source_revision_id,update_mode) VALUES($1,$2,$3,'subscription',$4,$5,$6,$7,$8,$9)").bind(id).bind(generation).bind((index+1) as i32).bind(source_id).bind(identity_epoch).bind(external_node_id).bind(node_version_id).bind(source_revision_id).bind(update_mode).execute(&mut **tx).await?;
            }
        }
    }
    Ok(())
}
pub(crate) async fn referenced_servers(
    connection: &mut PgConnection,
    id: i64,
) -> ApiResult<Vec<i64>> {
    Ok(sqlx::query_scalar("SELECT DISTINCT server_id FROM (SELECT n.server_id FROM singbox_chains c JOIN nodes n ON n.id=c.entry_node_id WHERE c.id=$1 UNION SELECT h.managed_server_id FROM singbox_ordered_chain_hops h JOIN singbox_chains c ON c.id=h.chain_id WHERE c.id=$1 AND h.kind='managed' AND (h.generation=ANY(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) OR EXISTS(SELECT 1 FROM unnest(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) AS selected(generation) WHERE selected.generation IS NOT NULL AND NOT EXISTS(SELECT 1 FROM singbox_ordered_chain_versions v WHERE v.chain_id=c.id AND v.generation=selected.generation))) UNION SELECT n.server_id FROM singbox_chains c JOIN nodes n ON n.id=c.exit_node_id WHERE c.id=$1) refs ORDER BY server_id").bind(id).fetch_all(connection).await?)
}
// This checks the managed runtime dependencies of an already frozen lineage.
// Archiving, replacing or losing a subscription source stops new references and
// follow updates in external_hop/follow_updates; it does not rewrite or retire
// an applied pinned snapshot. Runtime confirmation is checked separately.
pub(crate) async fn chain_is_structurally_available(
    connection: &mut PgConnection,
    id: i64,
) -> ApiResult<bool> {
    let ids:Vec<i64>=sqlx::query_scalar("SELECT c.entry_node_id FROM singbox_chains c WHERE c.id=$1 AND c.deleted_at IS NULL UNION SELECT h.managed_node_id FROM singbox_ordered_chain_hops h JOIN singbox_chains c ON c.id=h.chain_id WHERE c.id=$1 AND c.deleted_at IS NULL AND h.kind='managed' AND h.generation=ANY(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation])").bind(id).fetch_all(&mut *connection).await?;
    if ids.is_empty() {
        return Ok(false);
    }
    let query = format!(
        "SELECT COUNT(*) FROM nodes n JOIN servers s ON s.id=n.server_id LEFT JOIN server_plugins p ON p.server_id=s.id AND p.plugin='sing-box' WHERE n.id=ANY($1) AND n.deleted_at IS NULL AND n.enabled AND s.deleted_at IS NULL AND ({}) IS NOT NULL",
        super::super::settings::SOURCE_SQL
    );
    let count: i64 = sqlx::query_scalar(&query)
        .bind(&ids)
        .fetch_one(&mut *connection)
        .await?;
    if count != ids.len() as i64 {
        return Ok(false);
    }
    let rows = sqlx::query_as::<_, super::super::business::NodeRow>(&format!(
        "SELECT {} FROM nodes n WHERE n.id=ANY($1)",
        super::super::business::NODE_COLUMNS
    ))
    .bind(ids)
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows.iter().all(|row| {
        row.model(vec![])
            .is_ok_and(|node| node.protocol_config.is_reality() && node.enabled)
            && super::super::business::validate_node(row).is_ok()
    }))
}
pub(crate) async fn source_dependencies(
    connection: &mut PgConnection,
    source_id: i64,
) -> ApiResult<Vec<SourceDependency>> {
    Ok(sqlx::query_as("SELECT c.id AS chain_id,c.name AS chain_name,h.generation,CASE WHEN h.generation=c.applied_generation THEN 'applied' WHEN h.generation=c.candidate_generation THEN 'candidate' ELSE 'recovery' END AS state,h.position AS hop_position,h.external_node_id,h.node_version_id,h.identity_epoch FROM singbox_chains c JOIN singbox_ordered_chain_hops h ON h.chain_id=c.id WHERE h.source_id=$1 AND (c.deleted_at IS NULL OR c.phase<>'retired') AND h.generation=ANY(ARRAY[c.applied_generation,c.candidate_generation,c.recovery_generation]) ORDER BY c.id,h.generation,h.position").bind(source_id).fetch_all(connection).await?)
}
/// Share exact mutation ownership with the public configuration-lock projection.
/// Dedicated entries and every saved hop remain owned when any selected
/// immutable generation is missing; confirmed retirement releases that fallback.
pub(crate) async fn node_configuration_references(
    connection: &mut PgConnection,
    node_ids: &[i64],
) -> ApiResult<Vec<(i64, i64, String)>> {
    Ok(sqlx::query_as(
        "SELECT DISTINCT node_id,id,name FROM (
            SELECT c.entry_node_id AS node_id,c.id,c.name FROM singbox_chains c
            WHERE c.deleted_at IS NULL OR c.phase<>'retired'
            UNION ALL
            SELECT c.exit_node_id AS node_id,c.id,c.name FROM singbox_live_chains c
            UNION ALL
            SELECT h.managed_node_id AS node_id,c.id,c.name FROM singbox_chain_hops h
            JOIN singbox_live_chains c ON c.id=h.chain_id
            UNION ALL
            SELECT h.managed_node_id AS node_id,c.id,c.name FROM singbox_ordered_chain_hops h
            JOIN singbox_chains c ON c.id=h.chain_id
            WHERE (c.deleted_at IS NULL OR c.phase<>'retired')
                AND (h.generation=ANY(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) OR EXISTS(SELECT 1 FROM unnest(ARRAY[c.desired_generation,c.applied_generation,c.candidate_generation,c.recovery_generation]) AS selected(generation) WHERE selected.generation IS NOT NULL AND NOT EXISTS(SELECT 1 FROM singbox_ordered_chain_versions v WHERE v.chain_id=c.id AND v.generation=selected.generation)))
        ) refs WHERE node_id=ANY($1) ORDER BY node_id,id",
    )
    .bind(node_ids)
    .fetch_all(connection)
    .await?)
}

pub(crate) async fn ensure_node_edit_safe(
    tx: &mut Transaction<'_, Postgres>,
    node: &super::super::business::NodeRow,
    previous: &super::super::business::NodeRow,
) -> ApiResult<()> {
    let referenced = !node_configuration_references(tx, &[node.id])
        .await?
        .is_empty();
    if referenced
        && (node.port != previous.port
            || node.protocol != previous.protocol
            || node.protocol_config != previous.protocol_config
            || node.private_key != previous.private_key
            || node.public_key != previous.public_key
            || node.short_id != previous.short_id
            || node.public_host != previous.public_host
            || node.sni != previous.sni
            || node.settings != previous.settings)
    {
        return Err(ApiError::Conflict("该端点仍被有序路径引用，无法同时保留旧监听/认证参数；请创建替代节点和链路后明确调整策略".into()));
    }
    Ok(())
}
pub(crate) fn probe_target(state: &AppState) -> ApiResult<String> {
    let url = reqwest::Url::parse(&state.config.public_url)
        .map_err(|_| ApiError::Conflict("有序链路需要面板自己的 HTTPS 公共地址".into()))?;
    let raw = &state.config.public_url;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
        || url.path() != "/"
        || url.port() == Some(0)
        || raw.contains('@')
        || raw.contains('\\')
        || raw.chars().any(char::is_control)
    {
        return Err(ApiError::Conflict(
            "有序链路需要没有凭据、路径前缀或查询参数的面板 HTTPS 根地址".into(),
        ));
    }
    Ok(format!("{}/health", url.as_str().trim_end_matches('/')))
}
pub(crate) async fn reserve_probe_capacity(
    state: &AppState,
    connection: &mut PgConnection,
    server: i64,
) -> ApiResult<()> {
    let target = probe_target(state)?;
    let conflict:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE server_id=$1 AND port=18086 AND deleted_at IS NULL)").bind(server).fetch_one(&mut *connection).await?;
    if conflict {
        return Err(ApiError::Conflict(format!(
            "服务器 #{server} 的 18086 端口已占用，不能启用路径私有控制接口"
        )));
    }
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM singbox_chains c JOIN nodes n ON n.id=c.entry_node_id WHERE n.server_id=$1 AND c.path_kind='ordered' AND (c.deleted_at IS NULL OR c.phase<>'retired')").bind(server).fetch_one(connection).await?;
    // Reserve both probes for both applied and candidate generations. Retained managed
    // identities do not add controller bindings; capacity cannot fail after user switch.
    if count > 64
        || (count as usize)
            .saturating_mul(4)
            .saturating_mul(target.len() + 300)
            .saturating_add(2048)
            > 64 * 1024
    {
        return Err(ApiError::Conflict(
            "入口服务器的签名路径探测计划已达到容量上限，请使用另一入口服务器".into(),
        ));
    }
    Ok(())
}
pub(crate) async fn require_capabilities(
    tx: &mut Transaction<'_, Postgres>,
    servers: &BTreeSet<i64>,
) -> ApiResult<()> {
    for id in servers {
        let capabilities: Value =
            sqlx::query_scalar("SELECT capabilities FROM servers WHERE id=$1")
                .bind(id)
                .fetch_one(&mut **tx)
                .await?;
        for capability in [
            "runtime:checkpoint-v1",
            "runtime:barrier-v1",
            "runtime:path-probe-v1",
        ] {
            if !capabilities
                .as_array()
                .is_some_and(|items| items.iter().any(|value| value.as_str() == Some(capability)))
            {
                return Err(ApiError::Conflict(format!(
                    "服务器 #{id} 尚不支持完整路径确认、探测和恢复屏障，请升级 Agent"
                )));
            }
        }
    }
    Ok(())
}
pub(crate) async fn pin_runtimes(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    chain_id: i64,
    generation: i64,
    frozen: &FrozenVersion,
) -> ApiResult<()> {
    let mut servers = BTreeSet::from([frozen.entry.server_id]);
    for hop in &frozen.hops {
        if let FrozenHop::Managed { endpoint, .. } = hop {
            servers.insert(endpoint.server_id);
        }
    }
    for server in servers {
        let info: Value = sqlx::query_scalar("SELECT static_info FROM servers WHERE id=$1")
            .bind(server)
            .fetch_one(&mut **tx)
            .await?;
        let artifact = super::super::agent::runtime_artifact(state, &info)
            .await
            .map_err(|_| {
                ApiError::Conflict(format!(
                    "服务器 #{server} 缺少已验签的固定 sing-box 1.14.2 制品或平台信息"
                ))
            })?;
        let incompatible:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_chain_runtime_requirements p JOIN singbox_chains c ON c.id=p.chain_id WHERE p.server_id=$1 AND (c.deleted_at IS NULL OR c.phase<>'retired') AND p.generation=ANY(ARRAY[c.applied_generation,c.candidate_generation,c.recovery_generation]) AND p.artifact_sha256<>$2)").bind(server).bind(&artifact.sha256).fetch_one(&mut **tx).await?;
        if incompatible {
            return Err(ApiError::Conflict(format!(
                "服务器 #{server} 仍有依赖另一固定制品的链路，请先完成原代数撤销"
            )));
        }
        sqlx::query("INSERT INTO singbox_chain_runtime_requirements(chain_id,generation,server_id,runtime_version,artifact_sha256,artifact) VALUES($1,$2,$3,'1.14.2',$4,$5)").bind(chain_id).bind(generation).bind(server).bind(&artifact.sha256).bind(json!(artifact)).execute(&mut **tx).await?;
    }
    Ok(())
}

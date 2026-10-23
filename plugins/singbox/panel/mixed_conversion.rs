//! Converts mixed chains into ordered chains, one chain at a time, when an
//! administrator asks for it from the chain detail (ADR 0079 phase 3, step
//! S1d). A conversion is a real switch on the devices, not a byte-identical
//! takeover:
//!
//! - `preparing`: the chain stays mixed and its active generation keeps routing
//!   the entry. The ordered lifecycle prepares and probes an ordered candidate
//!   next to it without routing.
//! - `switched`: the candidate probe passed. The chain is ordered and its entry
//!   routes the candidate; the old generation stays on its dependencies.
//! - `completed`: the ordered lifecycle left the switching phases; the old
//!   generation is retired and only its tombstones remain.
//! - `reverted`: a failure before the switch returned the chain to mixed.

use crate::{
    AppState, auth,
    business::{self, NODE_COLUMNS, NodeRow},
    error::{ApiError, ApiResult},
    legacy_takeover::compiled,
    ordered_paths::{
        lifecycle,
        models::{CHAIN_COLUMNS, ChainRow, FrozenHop, FrozenVersion},
        storage,
    },
};
use apsp_compiler::paths::{self, Hop, Path};
use apsp_protocol::now_timestamp;
use axum::{
    Json,
    extract::{Path as Id, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, Postgres, Row, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

/// Ordered phases in which the entry routes the candidate while the old mixed
/// generation is still kept on its dependencies.
const SWITCHING: [&str; 3] = ["switching_entry", "probing_switched", "fixing_barrier"];

#[derive(Serialize)]
pub struct Check {
    pub chain_id: i64,
    pub ready: bool,
    /// Fixed codes plus hop positions; never addresses or credentials.
    pub reasons: Vec<String>,
    pub messages: Vec<String>,
    pub mixed_generation: Option<i64>,
    pub ordered_generation: Option<i64>,
}

#[derive(Serialize, FromRow)]
pub struct Conversion {
    pub state: String,
    pub mixed_generation: i64,
    pub ordered_generation: i64,
    pub attempts: i32,
    pub started_at: i64,
    pub switched_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Start {
    /// The mixed active generation the administrator looked at.
    pub expected_generation: i64,
}

fn message(reason: &str, detail: Option<&str>) -> String {
    if let Some((position, kind)) = reason
        .strip_prefix("hop_")
        .and_then(|rest| rest.split_once('_'))
    {
        let what = match kind {
            "deleted" => "受管节点已删除",
            "moved" => "受管节点已移到其他服务器",
            "disabled" => "受管节点已停用",
            "not_reality" => "受管节点不是 VLESS + Reality",
            "changed" => "受管节点参数已偏离链路保存的快照",
            "unreadable" => "受管节点参数无法解析",
            "unavailable" => "受管节点无法冻结为端点版本",
            "unmapped" => "订阅节点没有对应的已迁移节点或版本",
            "subscription_unavailable" => {
                "订阅节点已缺失、来源已归档或节点身份不唯一，请刷新来源后重试"
            }
            _ => "无法转换",
        };
        return format!("第 {position} 段：{what}");
    }
    match reason {
        "not_mixed" => "只有混合链路可以转换".into(),
        "deleted" => "链路已删除".into(),
        "mixed_update_pending" => "混合链路正在应用新版本，请等待完成后再转换".into(),
        "conversion_in_progress" => "链路已在转换中".into(),
        "generation_changed" => "链路版本已经变化，请刷新后重新确认".into(),
        "no_active_generation" => "链路还没有已应用的版本".into(),
        "mixed_not_ready" => {
            "当前混合路径尚未在全部服务器确认可用；转换需要从可用的路径开始".into()
        }
        "entry_direct_grants" => {
            "入口仍有直接授权；有序链路只接受整条链路的授权，请先改为通过策略组授权".into()
        }
        "entry_policy_node_grants" => "策略组把入口当作普通节点授权；请改为授权整条链路".into(),
        "probe_target_invalid" => "有序链路需要没有路径前缀或查询参数的面板 HTTPS 根地址".into(),
        "generation_unreadable" => "混合链路的版本记录无法读取".into(),
        "generation_mismatch" => "混合链路的版本与分段记录不一致".into(),
        "sources_not_migrated" => "链路含订阅段，需要先完成订阅来源迁移".into(),
        "entry_unavailable" => "入口节点无法冻结为端点版本".into(),
        "capabilities_missing" => {
            "参与的服务器尚不支持路径确认、探测和恢复屏障，请先升级 Agent".into()
        }
        "candidate_rejected" => format!("有序候选无法创建：{}", detail.unwrap_or("未知原因")),
        other => other.into(),
    }
}

/// Freezes the ordered candidate of a mixed generation: managed hops become
/// endpoint versions, subscription hops the migrated ordered node versions.
async fn candidate(
    tx: &mut Transaction<'_, Postgres>,
    chain: &ChainRow,
    generation: i64,
    reasons: &mut Vec<String>,
) -> ApiResult<Option<FrozenVersion>> {
    let raw: Value = sqlx::query_scalar(
        "SELECT path_json FROM singbox_chain_versions WHERE chain_id=$1 AND generation=$2",
    )
    .bind(chain.id)
    .bind(generation)
    .fetch_one(&mut **tx)
    .await?;
    let Ok(path) = serde_json::from_value::<Path>(raw) else {
        reasons.push("generation_unreadable".into());
        return Ok(None);
    };
    let rows = sqlx::query("SELECT position,source_id,update_mode FROM singbox_chain_hops WHERE chain_id=$1 AND generation=$2 ORDER BY position")
        .bind(chain.id)
        .bind(generation)
        .fetch_all(&mut **tx)
        .await?;
    if rows.len() != path.hops.len()
        || path.entry_node_id != chain.entry_node_id
        || rows
            .iter()
            .enumerate()
            .any(|(index, row)| row.get::<i32, _>("position") != index as i32)
    {
        reasons.push("generation_mismatch".into());
        return Ok(None);
    }
    let migrated: bool = sqlx::query_scalar("SELECT singbox_sources_migrated()")
        .fetch_one(&mut **tx)
        .await?;
    let mut hops = Vec::new();
    for (index, (row, hop)) in rows.iter().zip(&path.hops).enumerate() {
        let label = index + 1;
        match hop {
            Hop::Managed {
                server_id,
                endpoint,
                ..
            } => {
                let node: Option<NodeRow> = sqlx::query_as(&format!(
                    "SELECT {NODE_COLUMNS} FROM nodes n WHERE n.id=$1 AND n.deleted_at IS NULL"
                ))
                .bind(endpoint.id)
                .fetch_optional(&mut **tx)
                .await?;
                let Some(node) = node else {
                    reasons.push(format!("hop_{label}_deleted"));
                    continue;
                };
                let before = reasons.len();
                if node.server_id != *server_id {
                    reasons.push(format!("hop_{label}_moved"));
                }
                if !node.enabled {
                    reasons.push(format!("hop_{label}_disabled"));
                }
                if node.protocol != "vless-reality" {
                    reasons.push(format!("hop_{label}_not_reality"));
                }
                match node.model(vec![]) {
                    Ok(current) if compiled(&current)? == compiled(endpoint)? => {}
                    Ok(_) => reasons.push(format!("hop_{label}_changed")),
                    Err(_) => reasons.push(format!("hop_{label}_unreadable")),
                }
                if reasons.len() > before {
                    continue;
                }
                match storage::freeze_endpoint(tx, node.id).await {
                    Ok(endpoint) => hops.push(FrozenHop::Managed {
                        endpoint: Box::new(endpoint),
                        relay_uuid: Uuid::new_v4(),
                    }),
                    Err(ApiError::BadRequest(_) | ApiError::Conflict(_) | ApiError::NotFound) => {
                        reasons.push(format!("hop_{label}_unavailable"))
                    }
                    Err(error) => return Err(error),
                }
            }
            Hop::External {
                node_id,
                version_id,
                ..
            } => {
                if !migrated {
                    if !reasons
                        .iter()
                        .any(|reason| reason == "sources_not_migrated")
                    {
                        reasons.push("sources_not_migrated".into());
                    }
                    continue;
                }
                let mode: String = row.get("update_mode");
                let mapped: Option<(i64, Uuid, Uuid, Option<Uuid>)> = sqlx::query_as("SELECT n.source_id,n.id,v.id,n.latest_version FROM singbox_ordered_external_nodes n JOIN singbox_ordered_external_node_versions v ON v.node_id=n.id JOIN singbox_source_id_map m ON m.b_source_id=n.source_id WHERE m.a_source_id=$1 AND n.public_id=$2 AND v.public_id=$3")
                    .bind(row.get::<i64, _>("source_id")).bind(node_id).bind(version_id).fetch_optional(&mut **tx).await?;
                let Some((source, node, version, latest)) = mapped else {
                    reasons.push(format!("hop_{label}_unmapped"));
                    continue;
                };
                // Numbered sources froze at the source migration, so a following
                // mixed hop could no longer follow its node; the ordered hop
                // resumes from the node's latest version.
                let version = if mode == "follow_node" {
                    latest.unwrap_or(version)
                } else {
                    version
                };
                match storage::external_hop(tx, source, node, version, &mode).await {
                    Ok(hop) => hops.push(hop),
                    Err(ApiError::BadRequest(_) | ApiError::Conflict(_) | ApiError::NotFound) => {
                        reasons.push(format!("hop_{label}_subscription_unavailable"))
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }
    let entry = match storage::freeze_endpoint(tx, chain.entry_node_id).await {
        Ok(entry) => Some(entry),
        Err(ApiError::BadRequest(_) | ApiError::Conflict(_) | ApiError::NotFound) => {
            reasons.push("entry_unavailable".into());
            None
        }
        Err(error) => return Err(error),
    };
    Ok(match entry {
        Some(entry) if reasons.is_empty() => Some(FrozenVersion {
            entry,
            hops,
            legacy_relay_uuid: None,
        }),
        _ => None,
    })
}

/// Every check, then the ordered candidate in the same transaction. A precheck
/// rolls everything back; a start commits only when nothing failed.
async fn run(state: &AppState, id: i64, expected: Option<i64>) -> ApiResult<Check> {
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let chain: ChainRow = sqlx::query_as(&format!(
        "SELECT {CHAIN_COLUMNS} FROM singbox_chains WHERE id=$1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let (active, pending): (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT active_generation,pending_generation FROM singbox_chains WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    let mut reasons = Vec::new();
    let mut detail = None;
    let mut ordered = None;
    if chain.path_kind != "mixed" {
        reasons.push("not_mixed".into());
    }
    if chain.deleted_at.is_some() {
        reasons.push("deleted".into());
    }
    if pending.is_some() {
        reasons.push("mixed_update_pending".into());
    }
    let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_mixed_conversions WHERE chain_id=$1 AND state IN ('preparing','switched'))")
        .bind(id).fetch_one(&mut *tx).await?;
    if busy {
        reasons.push("conversion_in_progress".into());
    }
    if expected.is_some_and(|expected| Some(expected) != active) {
        reasons.push("generation_changed".into());
    }
    let ready: bool = sqlx::query_scalar("SELECT singbox_path_ready($1)")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if !ready && chain.path_kind == "mixed" {
        reasons.push("mixed_not_ready".into());
    }
    // Ordered entries accept only chain grants; any other grant would be dropped.
    let (direct, policy): (bool, bool) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM accesses WHERE node_id=$1 AND direct_grant),EXISTS(SELECT 1 FROM singbox_policy_nodes WHERE node_id=$1)")
        .bind(chain.entry_node_id).fetch_one(&mut *tx).await?;
    if direct {
        reasons.push("entry_direct_grants".into());
    }
    if policy {
        reasons.push("entry_policy_node_grants".into());
    }
    if storage::probe_target(state).is_err() {
        reasons.push("probe_target_invalid".into());
    }
    let frozen = match active {
        Some(generation) if chain.path_kind == "mixed" => {
            candidate(&mut tx, &chain, generation, &mut reasons).await?
        }
        None if chain.path_kind == "mixed" => {
            reasons.push("no_active_generation".into());
            None
        }
        _ => None,
    };
    if let Some(frozen) = frozen.filter(|_| reasons.is_empty()) {
        let mut servers = BTreeSet::from([frozen.entry.server_id]);
        for hop in &frozen.hops {
            if let FrozenHop::Managed { endpoint, .. } = hop {
                servers.insert(endpoint.server_id);
            }
        }
        if storage::require_capabilities(&mut tx, &servers)
            .await
            .is_err()
        {
            reasons.push("capabilities_missing".into());
        } else {
            match start(state, &mut tx, &chain, frozen).await {
                Ok(generation) => ordered = Some(generation),
                Err(ApiError::BadRequest(reason) | ApiError::Conflict(reason)) => {
                    reasons.push("candidate_rejected".into());
                    detail = Some(reason);
                }
                Err(error) => return Err(error),
            }
        }
    }
    let ready = reasons.is_empty();
    if ready && expected.is_some() {
        tx.commit().await?;
    } else {
        tx.rollback().await?;
    }
    Ok(Check {
        chain_id: id,
        ready,
        messages: reasons
            .iter()
            .map(|reason| message(reason, detail.as_deref()))
            .collect(),
        reasons,
        mixed_generation: active,
        ordered_generation: ordered,
    })
}

/// Starts the ordered candidate and records the conversion and the tombstones.
async fn start(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    chain: &ChainRow,
    frozen: FrozenVersion,
) -> ApiResult<i64> {
    let (active, latest): (i64, i64) = sqlx::query_as("SELECT c.active_generation,(SELECT MAX(generation) FROM singbox_chain_versions WHERE chain_id=c.id) FROM singbox_chains c WHERE c.id=$1")
        .bind(chain.id).fetch_one(&mut **tx).await?;
    // Ordered generations continue after every mixed one, so tags, identities
    // and evidence of the two lineages never share a generation number.
    let chain: ChainRow = sqlx::query_as(&format!("UPDATE singbox_chains SET desired_generation=GREATEST(desired_generation,$2) WHERE id=$1 RETURNING {CHAIN_COLUMNS}"))
        .bind(chain.id).bind(latest).fetch_one(&mut **tx).await?;
    let generation = lifecycle::start_conversion_candidate(state, tx, &chain, frozen).await?;
    let at = now_timestamp();
    sqlx::query("INSERT INTO singbox_mixed_conversions(chain_id,mixed_generation,ordered_generation,state,started_at) VALUES($1,$2,$3,'preparing',$4) ON CONFLICT(chain_id) DO UPDATE SET mixed_generation=EXCLUDED.mixed_generation,ordered_generation=EXCLUDED.ordered_generation,state='preparing',attempts=singbox_mixed_conversions.attempts+1,started_at=EXCLUDED.started_at,switched_at=NULL,finished_at=NULL,last_error=NULL")
        .bind(chain.id).bind(active).bind(generation).bind(at).execute(&mut **tx).await?;
    // Every server a mixed generation ever used keeps the highest generation as
    // its floor: a device may have committed any issued generation.
    sqlx::query("INSERT INTO singbox_retired_path_scopes(server_id,scope,floor,chain_id,recorded_at) SELECT server_id,$2,MAX(generation),$1,$3 FROM (SELECT generation,(path_json->>'entry_server_id')::bigint AS server_id FROM singbox_chain_versions WHERE chain_id=$1 UNION ALL SELECT generation,managed_server_id FROM singbox_chain_hops WHERE chain_id=$1 AND managed_server_id IS NOT NULL) used GROUP BY server_id ON CONFLICT(server_id,scope) DO UPDATE SET floor=GREATEST(singbox_retired_path_scopes.floor,EXCLUDED.floor),recorded_at=EXCLUDED.recorded_at")
        .bind(chain.id).bind(paths::scope(chain.id)).bind(at).execute(&mut **tx).await?;
    Ok(generation)
}

/// Returns a converting chain to mixed before its entry switched. Its mixed
/// generation never stopped routing; the candidate leaves every device.
pub(crate) async fn revert(
    tx: &mut Transaction<'_, Postgres>,
    chain: &ChainRow,
    reason: &str,
) -> ApiResult<()> {
    let servers = storage::referenced_servers(tx, chain.id).await?;
    sqlx::query("UPDATE singbox_chains SET candidate_generation=NULL,recovery_generation=NULL,applied_generation=NULL,phase='legacy',route_enabled=(deleted_at IS NULL),restore_step=NULL,last_error=$2 WHERE id=$1 AND path_kind='mixed'")
        .bind(chain.id).bind(reason).execute(&mut **tx).await?;
    sqlx::query("UPDATE singbox_mixed_conversions SET state='reverted',finished_at=$3,last_error=$2 WHERE chain_id=$1 AND state='preparing'")
        .bind(chain.id).bind(reason).bind(now_timestamp()).execute(&mut **tx).await?;
    business::mark_dirty(tx, &servers).await?;
    Ok(())
}

/// Hands the entry to the ordered candidate once its probe passed. Returns
/// false when a grant appeared that an ordered entry would drop; the chain is
/// then reverted instead.
pub(crate) async fn switch(
    tx: &mut Transaction<'_, Postgres>,
    chain: &ChainRow,
) -> ApiResult<bool> {
    let grants: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accesses WHERE node_id=$1 AND direct_grant) OR EXISTS(SELECT 1 FROM singbox_policy_nodes WHERE node_id=$1)")
        .bind(chain.entry_node_id).fetch_one(&mut **tx).await?;
    if grants {
        revert(
            tx,
            chain,
            "转换期间入口出现了直接授权或节点策略授权，已保留混合链路；请改为授权整条链路后重新转换",
        )
        .await?;
        return Ok(false);
    }
    sqlx::query("UPDATE singbox_chains SET path_kind='ordered',last_granted=EXISTS(SELECT 1 FROM singbox_eligible_accesses($2) WHERE node_id=entry_node_id) WHERE id=$1")
        .bind(chain.id).bind(now_timestamp()).execute(&mut **tx).await?;
    sqlx::query("UPDATE singbox_mixed_conversions SET state='switched',switched_at=$2 WHERE chain_id=$1 AND state='preparing'")
        .bind(chain.id).bind(now_timestamp()).execute(&mut **tx).await?;
    Ok(true)
}

/// Retires the old mixed generation once the ordered chain left the switching
/// phases: after its recovery barrier, or when it failed, retires or restores.
pub(crate) async fn settle(tx: &mut Transaction<'_, Postgres>, chain: &ChainRow) -> ApiResult<()> {
    if SWITCHING.contains(&chain.phase.as_str()) {
        return Ok(());
    }
    let generation: Option<i64> = sqlx::query_scalar("UPDATE singbox_mixed_conversions SET state='completed',finished_at=$2 WHERE chain_id=$1 AND state='switched' RETURNING mixed_generation")
        .bind(chain.id).bind(now_timestamp()).fetch_optional(&mut **tx).await?;
    if let Some(generation) = generation {
        let servers: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT managed_server_id FROM singbox_chain_hops WHERE chain_id=$1 AND generation=$2 AND managed_server_id IS NOT NULL ORDER BY 1")
            .bind(chain.id).bind(generation).fetch_all(&mut **tx).await?;
        business::mark_dirty(tx, &servers).await?;
    }
    Ok(())
}

pub(crate) async fn conversion(
    connection: &mut sqlx::PgConnection,
    id: i64,
) -> ApiResult<Option<Conversion>> {
    Ok(sqlx::query_as("SELECT state,mixed_generation,ordered_generation,attempts,started_at,switched_at,finished_at,last_error FROM singbox_mixed_conversions WHERE chain_id=$1")
        .bind(id).fetch_optional(connection).await?)
}

/// Runs every check and the candidate creation, then rolls back.
pub async fn precheck(
    State(state): State<AppState>,
    headers: HeaderMap,
    Id(id): Id<i64>,
) -> ApiResult<Json<Check>> {
    auth::require_admin(&state, &headers).await?;
    Ok(Json(run(&state, id, None).await?))
}

pub async fn convert(
    State(state): State<AppState>,
    headers: HeaderMap,
    Id(id): Id<i64>,
    Json(request): Json<Start>,
) -> ApiResult<(StatusCode, Json<Check>)> {
    auth::require_admin(&state, &headers).await?;
    let check = run(&state, id, Some(request.expected_generation)).await?;
    if !check.ready {
        return Err(ApiError::Conflict(format!(
            "链路暂不能转换：{}",
            check.messages.join("；")
        )));
    }
    Ok((StatusCode::ACCEPTED, Json(check)))
}

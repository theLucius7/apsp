use super::{models::*, storage};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    runtime_control,
};
use apsp_protocol::{
    RuntimeCheckpoint, RuntimePathProbeRequest, RuntimePathProbeResult,
    RuntimeRecoveryBarrierRequest, now_timestamp,
};
use serde_json::{Value, json};
use sqlx::{PgConnection, Postgres, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

type FollowNodeFact = (
    i64,
    bool,
    Option<i64>,
    String,
    Option<Uuid>,
    Option<Uuid>,
    Option<Uuid>,
);

pub(crate) async fn start_candidate(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    chain: &ChainRow,
    frozen: FrozenVersion,
) -> ApiResult<i64> {
    prepare_candidate(state, tx, chain, frozen, false).await
}

/// The candidate of a mixed chain under conversion. The chain stays mixed, and
/// its mixed generation keeps routing, until the candidate passes its probe.
pub(crate) async fn start_conversion_candidate(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    chain: &ChainRow,
    frozen: FrozenVersion,
) -> ApiResult<i64> {
    prepare_candidate(state, tx, chain, frozen, true).await
}

async fn prepare_candidate(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    chain: &ChainRow,
    mut frozen: FrozenVersion,
    converting: bool,
) -> ApiResult<i64> {
    if chain.deleted_at.is_some() || chain.candidate_generation.is_some() {
        return Err(ApiError::Conflict(
            "链路已删除或仍有未完成候选，请等待当前应用确认".into(),
        ));
    }
    storage::probe_target(state)?;
    let generation = chain
        .desired_generation
        .checked_add(1)
        .ok_or_else(|| ApiError::Conflict("链路代数已达到上限".into()))?;
    let mut servers = BTreeSet::from([frozen.entry.server_id]);
    frozen.legacy_relay_uuid = None;
    for hop in &mut frozen.hops {
        if let FrozenHop::Managed {
            endpoint,
            relay_uuid,
        } = hop
        {
            servers.insert(endpoint.server_id);
            *relay_uuid = Uuid::new_v4();
        }
    }
    storage::require_capabilities(tx, &servers).await?;
    super::super::proxy_resources::lock_cleanup_servers(
        tx,
        &servers.iter().copied().collect::<Vec<_>>(),
    )
    .await?;
    let path = storage::compiler_path(chain.id, generation, &frozen, false);
    apsp_compiler::validate_path(&path)
        .map_err(|error| ApiError::BadRequest(format!("完整路径无法编译：{error}")))?;
    let capabilities = apsp_compiler::path_capabilities(&path)
        .map_err(|error| ApiError::BadRequest(format!("路径下层承载不兼容：{error}")))?;
    if !capabilities.tcp {
        return Err(ApiError::Conflict(
            "指定出站 HTTPS 探测不能确认仅 UDP 的路径".into(),
        ));
    }
    storage::save_version(
        tx,
        chain.id,
        generation,
        &frozen,
        false,
        &Capabilities {
            tcp: capabilities.tcp,
            udp: capabilities.udp,
        },
    )
    .await?;
    storage::pin_runtimes(state, tx, chain.id, generation, &frozen).await?;
    if converting {
        sqlx::query("UPDATE singbox_chains SET desired_generation=$2,candidate_generation=$2,applied_generation=NULL,recovery_generation=NULL,phase='preparing_dependencies',route_enabled=FALSE,restore_step=NULL,last_error=NULL WHERE id=$1 AND path_kind='mixed'").bind(chain.id).bind(generation).execute(&mut **tx).await?;
    } else {
        sqlx::query("UPDATE singbox_chains SET path_kind='ordered',exit_node_id=NULL,relay_uuid=NULL,desired_generation=$2,candidate_generation=$2,recovery_generation=applied_generation,phase='preparing_dependencies',restore_step=NULL,last_error=NULL WHERE id=$1").bind(chain.id).bind(generation).execute(&mut **tx).await?;
    }
    storage::reserve_probe_capacity(state, tx, frozen.entry.server_id).await?;
    super::publication::validate_candidate(tx, &path, &frozen).await?;
    super::super::business::mark_dirty(tx, &servers.into_iter().collect::<Vec<_>>()).await?;
    Ok(generation)
}
pub(crate) async fn candidate_for_endpoint_change(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
) -> ApiResult<i64> {
    let chain = storage::chain(tx, id, true).await?;
    let mut frozen = storage::version(tx, id, chain.desired_generation)
        .await?
        .snapshot;
    frozen.entry = storage::freeze_endpoint(tx, frozen.entry.node.id).await?;
    for hop in &mut frozen.hops {
        if let FrozenHop::Managed { endpoint, .. } = hop {
            **endpoint = storage::freeze_endpoint(tx, endpoint.node.id).await?;
        }
    }
    start_candidate(state, tx, &chain, frozen).await
}
async fn phase(tx: &mut Transaction<'_, Postgres>, chain: &ChainRow, value: &str) -> ApiResult<()> {
    sqlx::query("UPDATE singbox_chains SET phase=$2 WHERE id=$1")
        .bind(chain.id)
        .bind(value)
        .execute(&mut **tx)
        .await?;
    let servers = storage::referenced_servers(tx, chain.id).await?;
    super::super::business::mark_dirty(tx, &servers).await?;
    Ok(())
}
async fn probe_row(
    tx: &mut Transaction<'_, Postgres>,
    chain_id: i64,
    generation: i64,
    stage: &str,
    server: i64,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO singbox_path_probes(probe_id,chain_id,generation,stage,server_id,created_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(chain_id,generation,stage) DO NOTHING").bind(Uuid::new_v4()).bind(chain_id).bind(generation).bind(stage).bind(server).bind(now_timestamp()).execute(&mut **tx).await?;
    Ok(())
}
async fn checkpoint_for_stage(
    connection: &mut PgConnection,
    chain_id: i64,
    generation: i64,
    stage: &str,
    server: i64,
    required_generation: Option<i64>,
    absence: Option<i64>,
) -> ApiResult<Option<RuntimeCheckpoint>> {
    let expected:Option<(i64,String,Uuid,String)>=sqlx::query_as("SELECT revision,bundle_sha256,deployment_id,binding_digest FROM singbox_path_stage_deployments WHERE chain_id=$1 AND generation=$2 AND stage=$3 AND server_id=$4").bind(chain_id).bind(generation).bind(stage).bind(server).fetch_optional(&mut *connection).await?;
    let Some((revision, hash, deployment_id, digest)) = expected else {
        return Ok(None);
    };
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_module_status m JOIN servers s ON s.id=m.server_id JOIN runtime_module_checkpoints r ON r.server_id=m.server_id AND r.module=m.module WHERE m.server_id=$1 AND m.module='singbox' AND m.target_rev=$2 AND m.applied_rev=$2 AND m.healthy AND s.deleted_at IS NULL AND s.dirty_at IS NULL AND r.checkpoint_json->'binding'->>'bundle_sha256'=$3 AND r.checkpoint_json->'binding'->>'deployment_id'=$4 AND r.checkpoint_json->'binding'->>'binding_digest'=$5 AND (r.checkpoint_json->'binding'->>'revision')::bigint=$2 AND r.checkpoint_json->>'healthy'='true')").bind(server).bind(revision).bind(&hash).bind(deployment_id.to_string()).bind(&digest).fetch_one(&mut *connection).await?;
    if !valid {
        return Ok(None);
    }
    if let Some(required) = required_generation {
        let contains:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_path_deployment_dependencies d WHERE d.server_id=$1 AND d.module='singbox' AND d.revision=$2 AND d.chain_id=$3 AND d.generation=$4)").bind(server).bind(revision).bind(chain_id).bind(required).fetch_one(&mut *connection).await?;
        if !contains {
            return Ok(None);
        }
        let runtime:bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM singbox_chain_runtime_requirements p WHERE p.chain_id=$1 AND p.generation=$2 AND p.server_id=$3 AND NOT EXISTS(SELECT 1 FROM singbox_runtime_manifest_facts f WHERE f.server_id=$3 AND f.module='singbox' AND f.revision=$4 AND f.runtime_version=p.runtime_version AND f.artifact_sha256=p.artifact_sha256))").bind(chain_id).bind(required).bind(server).bind(revision).fetch_one(&mut *connection).await?;
        if !runtime {
            return Ok(None);
        }
    }
    if let Some(absent) = absence {
        let present:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_path_deployment_dependencies WHERE server_id=$1 AND module='singbox' AND revision=$2 AND chain_id=$3 AND generation=$4)").bind(server).bind(revision).bind(chain_id).bind(absent).fetch_one(&mut *connection).await?;
        if present {
            return Ok(None);
        }
    }
    let checkpoint:Value=sqlx::query_scalar("SELECT checkpoint_json FROM runtime_module_checkpoints WHERE server_id=$1 AND module='singbox'").bind(server).fetch_one(&mut *connection).await?;
    let observed: RuntimeCheckpoint =
        serde_json::from_value(checkpoint).map_err(anyhow::Error::from)?;
    sqlx::query("UPDATE singbox_path_stage_deployments SET checkpoint_request_id=(SELECT checkpoint_request_id FROM runtime_module_checkpoints WHERE server_id=$4 AND module='singbox'),observed_at=(SELECT verified_at FROM runtime_module_checkpoints WHERE server_id=$4 AND module='singbox') WHERE chain_id=$1 AND generation=$2 AND stage=$3 AND server_id=$4").bind(chain_id).bind(generation).bind(stage).bind(server).execute(connection).await?;
    Ok(Some(observed))
}
async fn stage_failed(
    connection: &mut PgConnection,
    chain_id: i64,
    generation: i64,
    stage: &str,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_path_stage_deployments d JOIN server_module_status m ON m.server_id=d.server_id AND m.module='singbox' WHERE d.chain_id=$1 AND d.generation=$2 AND d.stage=$3 AND m.target_rev=d.revision AND m.last_result_rev>=d.revision AND NOT m.healthy AND m.last_error IS NOT NULL)").bind(chain_id).bind(generation).bind(stage).fetch_one(connection).await?)
}
async fn fail_candidate(
    tx: &mut Transaction<'_, Postgres>,
    chain: &ChainRow,
    reason: &str,
) -> ApiResult<()> {
    if chain.path_kind == "mixed" {
        // A conversion that fails before its entry switched keeps the mixed route.
        return crate::mixed_conversion::revert(tx, chain, reason).await;
    }
    let unknown_barrier:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_control_requests q JOIN singbox_path_deployment_dependencies d ON d.server_id=q.server_id AND d.module=q.module AND d.revision=(q.request_json->'expected'->'binding'->>'revision')::bigint WHERE q.kind='barrier' AND d.chain_id=$1 AND d.generation=$2 AND d.role='entry' AND d.route_active AND NOT EXISTS(SELECT 1 FROM runtime_control_receipts f WHERE f.request_id=q.request_id))").bind(chain.id).bind(chain.candidate_generation).fetch_one(&mut **tx).await?;
    if unknown_barrier {
        // An orphan/expired command may already have committed its device floor.
        // Until its durable receipt arrives, preserve the switched candidate route
        // and both identity sets; a fresh probe failure cannot authorize a rollback.
        sqlx::query("UPDATE singbox_chains SET phase='fixing_barrier',restore_step=NULL,last_error='路径确认失败，但仍有恢复屏障等待设备持久回执；尚不清理旧身份或回退路径' WHERE id=$1").bind(chain.id).execute(&mut **tx).await?;
        let servers = storage::referenced_servers(tx, chain.id).await?;
        super::super::business::mark_dirty(tx, &servers).await?;
        return Ok(());
    }
    let recoverable = chain.phase != "restoring"
        && chain
            .applied_generation
            .is_some_and(|generation| generation >= chain.minimum_generation)
        && chain.minimum_generation
            < chain
                .candidate_generation
                .unwrap_or(chain.desired_generation);
    sqlx::query("UPDATE singbox_chains SET phase=$2,restore_step=$3,last_error=$4 WHERE id=$1")
        .bind(chain.id)
        .bind(if recoverable { "restoring" } else { "failed" })
        .bind(recoverable.then_some("entry"))
        .bind(reason)
        .execute(&mut **tx)
        .await?;
    let servers = storage::referenced_servers(tx, chain.id).await?;
    super::super::business::mark_dirty(tx, &servers).await?;
    Ok(())
}

async fn probe_matches_vector(
    connection: &mut PgConnection,
    id: i64,
    generation: i64,
    vector: &[(i64, RuntimeCheckpoint)],
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_path_probes p JOIN runtime_control_requests q ON q.request_id=p.request_id JOIN runtime_control_receipts f ON f.request_id=q.request_id WHERE p.chain_id=$1 AND p.generation=$2 AND p.stage='switched' AND p.state='verified' AND p.dependency_vector=$3 AND q.kind='probe' AND f.outcome='verified' AND f.result_json->>'success'='true' AND f.result_json->>'probe_id'=p.probe_id::text AND f.result_json->'observed'=q.request_json->'expected')").bind(id).bind(generation).bind(json!(vector)).fetch_one(connection).await?)
}

async fn finish_applied(
    tx: &mut Transaction<'_, Postgres>,
    chain: &ChainRow,
    generation: i64,
    stage_generation: i64,
    stage: &str,
    clear_error: bool,
) -> ApiResult<()> {
    // Finalization changes bookkeeping only: cleanup has already published the final
    // bytes and the complete vector was confirmed after the identities were removed.
    // Keep that exact binding as the initial applied-stage fact instead of making a
    // successful path dirty merely to recreate an equivalent stage record.
    sqlx::query("INSERT INTO singbox_path_stage_deployments(chain_id,generation,stage,server_id,role,hop_position,revision,bundle_sha256,deployment_id,binding_digest,checkpoint_request_id,observed_at) SELECT chain_id,$2,'applied',server_id,role,hop_position,revision,bundle_sha256,deployment_id,binding_digest,checkpoint_request_id,observed_at FROM singbox_path_stage_deployments WHERE chain_id=$1 AND generation=$3 AND stage=$4 ON CONFLICT(chain_id,generation,stage,server_id) DO UPDATE SET revision=EXCLUDED.revision,bundle_sha256=EXCLUDED.bundle_sha256,deployment_id=EXCLUDED.deployment_id,binding_digest=EXCLUDED.binding_digest,checkpoint_request_id=EXCLUDED.checkpoint_request_id,barrier_request_id=NULL,barrier_vector=NULL,observed_at=EXCLUDED.observed_at")
        .bind(chain.id).bind(generation).bind(stage_generation).bind(stage).execute(&mut **tx).await?;
    sqlx::query("UPDATE singbox_chains SET applied_generation=$2,candidate_generation=NULL,recovery_generation=NULL,phase='applied',restore_step=NULL,last_error=CASE WHEN $3 THEN NULL ELSE last_error END WHERE id=$1").bind(chain.id).bind(generation).bind(clear_error).execute(&mut **tx).await?;
    Ok(())
}
enum Action {
    Probe {
        chain_id: i64,
        generation: i64,
        phase: String,
        stage: String,
        server: i64,
        probe_id: Uuid,
        expected: RuntimeCheckpoint,
        vector: Vec<(i64, RuntimeCheckpoint)>,
    },
    Barrier {
        chain_id: i64,
        generation: i64,
        phase: String,
        stage: String,
        server: i64,
        expected: RuntimeCheckpoint,
        vector: Vec<(i64, RuntimeCheckpoint)>,
    },
}

async fn follow_updates(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    chain: &ChainRow,
) -> ApiResult<()> {
    let mut frozen = storage::version(tx, chain.id, chain.desired_generation)
        .await?
        .snapshot;
    let mut changed = false;
    for hop in &mut frozen.hops {
        let FrozenHop::Subscription {
            source_id,
            identity_epoch,
            external_node_id,
            node_version_id,
            update_mode,
            content_digest,
            ..
        } = hop
        else {
            continue;
        };
        if update_mode != "follow_node" {
            continue;
        }
        let latest:Option<FollowNodeFact>=sqlx::query_as("SELECT s.identity_epoch,s.archived,s.deleted_at,n.identity_state,n.latest_version,n.last_seen_revision,s.current_success_revision FROM singbox_ordered_subscription_sources s JOIN singbox_ordered_external_nodes n ON n.source_id=s.id WHERE s.id=$1 AND n.id=$2 FOR UPDATE OF s").bind(*source_id).bind(*external_node_id).fetch_optional(&mut **tx).await?;
        let Some((epoch, archived, deleted, identity, version, seen, current)) = latest else {
            continue;
        };
        if epoch != *identity_epoch
            || archived
            || deleted.is_some()
            || identity != "unique"
            || seen != current
            || current.is_none()
        {
            continue;
        }
        let Some(version) = version else {
            continue;
        };
        if version == *node_version_id {
            continue;
        }
        let candidate =
            storage::external_hop(tx, *source_id, *external_node_id, version, update_mode).await?;
        if let FrozenHop::Subscription {
            content_digest: new_digest,
            ..
        } = &candidate
            && new_digest == content_digest
        {
            continue;
        }
        *hop = candidate;
        changed = true;
    }
    if changed {
        start_candidate(state, tx, chain, frozen).await?;
    }
    Ok(())
}

async fn advance(state: &AppState, id: i64) -> ApiResult<Option<Action>> {
    let mut tx = state.pool.begin().await?;
    super::super::entitlements::lock(&mut tx).await?;
    let chain = storage::lifecycle_chain(&mut tx, id, true).await?;
    // A mixed chain here is under conversion and has not switched yet.
    let converting = chain.path_kind == "mixed";
    if (chain.path_kind != "ordered" && !converting) || chain.phase == "retired" {
        tx.commit().await?;
        return Ok(None);
    }
    if !converting {
        crate::mixed_conversion::settle(&mut tx, &chain).await?;
    }
    let servers = storage::referenced_servers(&mut tx, id).await?;
    super::super::proxy_resources::lock_cleanup_servers(&mut tx, &servers).await?;
    let live = storage::chain_is_structurally_available(&mut tx, id).await?;
    let granted: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_eligible_accesses($2) WHERE node_id=$1)",
    )
    .bind(chain.entry_node_id)
    .bind(now_timestamp())
    .fetch_one(&mut *tx)
    .await?;
    if chain.last_granted != granted {
        sqlx::query("UPDATE singbox_chains SET last_granted=$2 WHERE id=$1")
            .bind(id)
            .bind(granted)
            .execute(&mut *tx)
            .await?;
    }
    if converting {
        // Grants of a converting chain stay with its mixed route; losing the
        // chain or its resources returns it to mixed, which retires it.
        if !live || chain.deleted_at.is_some() {
            crate::mixed_conversion::revert(
                &mut tx,
                &chain,
                if chain.deleted_at.is_some() {
                    "链路在转换期间被删除，已撤下有序候选"
                } else {
                    "受管资源在转换期间停用或退役，已撤下有序候选"
                },
            )
            .await?;
            tx.commit().await?;
            return Ok(None);
        }
    } else if (!live || chain.deleted_at.is_some() || (chain.last_granted && !granted))
        && chain.phase != "retiring"
    {
        sqlx::query("UPDATE singbox_chains SET phase='retiring',route_enabled=FALSE,last_error=$2 WHERE id=$1").bind(id).bind(if chain.deleted_at.is_some(){"链路已删除，等待存活依赖撤销确认"}else if !live{"受管资源已停用或退役，等待撤销确认"}else{"用户资格已撤销，等待依赖撤销确认"}).execute(&mut *tx).await?;
        super::super::business::mark_dirty(&mut tx, &servers).await?;
        tx.commit().await?;
        return Ok(None);
    }
    let generation = if chain.phase == "applied" {
        chain.applied_generation.unwrap_or(chain.desired_generation)
    } else {
        chain
            .candidate_generation
            .unwrap_or(chain.desired_generation)
    };
    let selected = storage::version(&mut tx, id, generation).await?;
    // A legacy generation has no probe bindings; it is confirmed by server health.
    let legacy_generation = selected.legacy;
    let frozen = selected.snapshot;
    let entry = frozen.entry.server_id;
    let managed: Vec<i64> = frozen
        .hops
        .iter()
        .filter_map(|hop| match hop {
            FrozenHop::Managed { endpoint, .. } => Some(endpoint.server_id),
            _ => None,
        })
        .collect();
    let mut action = None;
    match chain.phase.as_str() {
        "preparing_dependencies" => {
            if stage_failed(&mut tx, id, generation, "preparing_dependencies").await? {
                fail_candidate(&mut tx, &chain, "受管依赖应用失败，未切换用户路径").await?;
            } else {
                let mut ready = true;
                for server in managed {
                    if checkpoint_for_stage(
                        &mut tx,
                        id,
                        generation,
                        "preparing_dependencies",
                        server,
                        Some(generation),
                        None,
                    )
                    .await?
                    .is_none()
                    {
                        ready = false;
                    }
                }
                if ready {
                    probe_row(&mut tx, id, generation, "candidate", entry).await?;
                    phase(&mut tx, &chain, "preparing_entry").await?;
                }
            }
        }
        "preparing_entry" | "switching_entry" => {
            if stage_failed(&mut tx, id, generation, &chain.phase).await? {
                fail_candidate(&mut tx, &chain, "入口配置应用失败，保留可恢复的已应用代数").await?;
            } else if checkpoint_for_stage(
                &mut tx,
                id,
                generation,
                &chain.phase,
                entry,
                Some(generation),
                None,
            )
            .await?
            .is_some()
            {
                phase(
                    &mut tx,
                    &chain,
                    if chain.phase == "preparing_entry" {
                        "probing_candidate"
                    } else {
                        "probing_switched"
                    },
                )
                .await?;
            }
        }
        "probing_candidate" | "probing_switched" => {
            let stage = if chain.phase == "probing_candidate" {
                "candidate"
            } else {
                "switched"
            };
            let deployment_stage = if stage == "candidate" {
                "preparing_entry"
            } else {
                "switching_entry"
            };
            if let Some(expected) = checkpoint_for_stage(
                &mut tx,
                id,
                generation,
                deployment_stage,
                entry,
                Some(generation),
                None,
            )
            .await?
            {
                action = Some(Action::Probe {
                    chain_id: id,
                    generation,
                    phase: chain.phase.clone(),
                    stage: stage.into(),
                    server: entry,
                    probe_id: probe_identifier(&mut tx, id, generation, stage).await?,
                    expected,
                    vector: vec![],
                });
            }
        }
        "fixing_barrier" => {
            if let Some(expected) = checkpoint_for_stage(
                &mut tx,
                id,
                generation,
                "switching_entry",
                entry,
                Some(generation),
                None,
            )
            .await?
            {
                action = Some(Action::Barrier {
                    chain_id: id,
                    generation,
                    phase: chain.phase.clone(),
                    stage: "switching_entry".into(),
                    server: entry,
                    expected,
                    vector: vec![],
                });
            }
        }
        "retiring_old" => {
            let mut ready = true;
            for server in &servers {
                let alive: bool =
                    sqlx::query_scalar("SELECT deleted_at IS NULL FROM servers WHERE id=$1")
                        .bind(server)
                        .fetch_one(&mut *tx)
                        .await?;
                if alive
                    && checkpoint_for_stage(
                        &mut tx,
                        id,
                        generation,
                        "retiring_old",
                        *server,
                        Some(generation),
                        chain.recovery_generation,
                    )
                    .await?
                    .is_none()
                {
                    ready = false;
                }
            }
            if ready && let Some(vector) = current_vector(&mut tx, id, generation).await? {
                if probe_matches_vector(&mut tx, id, generation, &vector).await? {
                    finish_applied(
                        &mut tx,
                        &chain,
                        generation,
                        generation,
                        "retiring_old",
                        true,
                    )
                    .await?;
                } else if let Some(expected) = checkpoint_for_stage(
                    &mut tx,
                    id,
                    generation,
                    "retiring_old",
                    entry,
                    Some(generation),
                    chain.recovery_generation,
                )
                .await?
                {
                    action = Some(Action::Probe {
                        chain_id: id,
                        generation,
                        phase: "retiring_old".into(),
                        stage: "switched".into(),
                        server: entry,
                        probe_id: probe_identifier(&mut tx, id, generation, "switched").await?,
                        expected,
                        vector,
                    });
                }
            }
        }
        "restoring" => {
            let old = chain
                .applied_generation
                .ok_or_else(|| ApiError::Conflict("没有可恢复的链路代数".into()))?;
            let old_version = storage::version(&mut tx, id, old).await?;
            match chain.restore_step.as_deref().unwrap_or("entry") {
                "entry" => {
                    if let Some(expected) = checkpoint_for_stage(
                        &mut tx,
                        id,
                        generation,
                        "restoring",
                        entry,
                        Some(old),
                        Some(generation),
                    )
                    .await?
                    {
                        sqlx::query("UPDATE singbox_chains SET restore_step=$2 WHERE id=$1")
                            .bind(id)
                            .bind(if old_version.legacy {
                                "barrier"
                            } else {
                                "probe"
                            })
                            .execute(&mut *tx)
                            .await?;
                        if old_version.legacy {
                            action = Some(Action::Barrier {
                                chain_id: id,
                                generation,
                                phase: "restoring".into(),
                                stage: "restoring".into(),
                                server: entry,
                                expected,
                                vector: vec![],
                            });
                        }
                    }
                }
                "probe" => {
                    if let Some(expected) = checkpoint_for_stage(
                        &mut tx,
                        id,
                        generation,
                        "restoring",
                        entry,
                        Some(old),
                        Some(generation),
                    )
                    .await?
                    {
                        action = Some(Action::Probe {
                            chain_id: id,
                            generation: old,
                            phase: "restoring".into(),
                            stage: "switched".into(),
                            server: entry,
                            probe_id: probe_identifier(&mut tx, id, old, "switched").await?,
                            expected,
                            vector: vec![],
                        });
                    }
                }
                "barrier" => {
                    if let Some(expected) = checkpoint_for_stage(
                        &mut tx,
                        id,
                        generation,
                        "restoring",
                        entry,
                        Some(old),
                        Some(generation),
                    )
                    .await?
                    {
                        action = Some(Action::Barrier {
                            chain_id: id,
                            generation,
                            phase: "restoring".into(),
                            stage: "restoring".into(),
                            server: entry,
                            expected,
                            vector: vec![],
                        });
                    }
                }
                "cleanup" => {
                    let mut ready = true;
                    for server in &servers {
                        let alive: bool = sqlx::query_scalar(
                            "SELECT deleted_at IS NULL FROM servers WHERE id=$1",
                        )
                        .bind(server)
                        .fetch_one(&mut *tx)
                        .await?;
                        if alive
                            && checkpoint_for_stage(
                                &mut tx,
                                id,
                                generation,
                                "restoring_cleanup",
                                *server,
                                Some(old),
                                Some(generation),
                            )
                            .await?
                            .is_none()
                        {
                            ready = false;
                        }
                    }
                    if ready && let Some(vector) = current_vector(&mut tx, id, old).await? {
                        if old_version.legacy
                            || probe_matches_vector(&mut tx, id, old, &vector).await?
                        {
                            finish_applied(
                                &mut tx,
                                &chain,
                                old,
                                generation,
                                "restoring_cleanup",
                                false,
                            )
                            .await?;
                        } else if let Some(expected) = checkpoint_for_stage(
                            &mut tx,
                            id,
                            generation,
                            "restoring_cleanup",
                            entry,
                            Some(old),
                            Some(generation),
                        )
                        .await?
                        {
                            action = Some(Action::Probe {
                                chain_id: id,
                                generation: old,
                                phase: "restoring".into(),
                                stage: "switched".into(),
                                server: entry,
                                probe_id: probe_identifier(&mut tx, id, old, "switched").await?,
                                expected,
                                vector,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        "retiring" => {
            let mut ready = true;
            for server in servers {
                let alive: bool =
                    sqlx::query_scalar("SELECT deleted_at IS NULL FROM servers WHERE id=$1")
                        .bind(server)
                        .fetch_one(&mut *tx)
                        .await?;
                if alive {
                    for absent in [
                        chain.applied_generation,
                        chain.candidate_generation,
                        chain.recovery_generation,
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if checkpoint_for_stage(
                            &mut tx,
                            id,
                            chain.desired_generation,
                            "retiring",
                            server,
                            None,
                            Some(absent),
                        )
                        .await?
                        .is_none()
                        {
                            ready = false;
                        }
                    }
                }
            }
            if ready {
                sqlx::query("UPDATE singbox_chains SET phase=$2,candidate_generation=NULL,recovery_generation=NULL,restore_step=NULL,applied_generation=CASE WHEN deleted_at IS NULL THEN applied_generation ELSE NULL END WHERE id=$1").bind(id).bind(if chain.deleted_at.is_some(){"retired"}else{"applied"}).execute(&mut *tx).await?;
            }
        }
        "applied" => {
            if granted && !chain.route_enabled {
                candidate_for_endpoint_change(state, &mut tx, id).await?;
            } else if chain.candidate_generation.is_none() {
                follow_updates(state, &mut tx, &chain).await?;
                let fresh = storage::chain(&mut tx, id, false).await?;
                if fresh.candidate_generation.is_none() && fresh.route_enabled {
                    let mut vector_ready = true;
                    let active_servers = BTreeSet::from_iter(std::iter::once(entry).chain(managed));
                    for server in active_servers {
                        if checkpoint_for_stage(
                            &mut tx,
                            id,
                            generation,
                            "applied",
                            server,
                            Some(generation),
                            None,
                        )
                        .await?
                        .is_none()
                        {
                            vector_ready = false;
                        }
                    }
                    if vector_ready
                        && !legacy_generation
                        && !qualified(&mut tx, id).await?
                        && let Some(expected) = checkpoint_for_stage(
                            &mut tx,
                            id,
                            generation,
                            "applied",
                            entry,
                            Some(generation),
                            None,
                        )
                        .await?
                    {
                        action = Some(Action::Probe {
                            chain_id: id,
                            generation,
                            phase: "applied".into(),
                            stage: "switched".into(),
                            server: entry,
                            probe_id: probe_identifier(&mut tx, id, generation, "switched").await?,
                            expected,
                            vector: vec![],
                        });
                    }
                }
            }
        }
        "failed" => {}
        _ => {}
    }
    if let Some(action) = action.as_mut() {
        let semantic_generation = if chain.phase == "restoring" {
            chain.applied_generation.unwrap_or(generation)
        } else {
            generation
        };
        if let Some(observations) = current_vector(&mut tx, id, semantic_generation).await? {
            match action {
                Action::Probe { vector, .. } | Action::Barrier { vector, .. } => {
                    *vector = observations;
                }
            }
        } else {
            tx.commit().await?;
            return Ok(None);
        }
    }
    tx.commit().await?;
    Ok(action)
}
async fn probe_identifier(
    connection: &mut PgConnection,
    id: i64,
    generation: i64,
    stage: &str,
) -> ApiResult<Uuid> {
    Ok(sqlx::query_scalar(
        "SELECT probe_id FROM singbox_path_probes WHERE chain_id=$1 AND generation=$2 AND stage=$3",
    )
    .bind(id)
    .bind(generation)
    .bind(stage)
    .fetch_one(connection)
    .await?)
}

async fn current_vector(
    connection: &mut PgConnection,
    id: i64,
    generation: i64,
) -> ApiResult<Option<Vec<(i64, RuntimeCheckpoint)>>> {
    let version = storage::version(connection, id, generation).await?;
    let mut servers = BTreeSet::from([version.snapshot.entry.server_id]);
    for hop in &version.snapshot.hops {
        if let FrozenHop::Managed { endpoint, .. } = hop {
            servers.insert(endpoint.server_id);
        }
    }
    let mut vector = Vec::new();
    for server in servers {
        let Ok(observed) =
            runtime_control::checked_target_checkpoint(connection, server, "singbox").await
        else {
            return Ok(None);
        };
        let contains:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_path_deployment_dependencies WHERE server_id=$1 AND module='singbox' AND revision=$2 AND chain_id=$3 AND generation=$4)").bind(server).bind(observed.binding.revision as i64).bind(id).bind(generation).fetch_one(&mut *connection).await?;
        if !contains {
            return Ok(None);
        }
        if !version.legacy {
            let prepared:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_chain_runtime_requirements p JOIN singbox_runtime_manifest_facts f ON f.server_id=p.server_id AND f.module='singbox' AND f.revision=$4 AND f.runtime_version=p.runtime_version AND f.artifact_sha256=p.artifact_sha256 WHERE p.chain_id=$1 AND p.generation=$2 AND p.server_id=$3)").bind(id).bind(generation).bind(server).bind(observed.binding.revision as i64).fetch_one(&mut *connection).await?;
            if !prepared {
                return Ok(None);
            }
        }
        vector.push((server, observed));
    }
    Ok(Some(vector))
}

/// Public subscriptions require the currently applied complete vector, never an old receipt
/// that happens to refer to the same logical chain or generation number.
pub(crate) async fn qualified(connection: &mut PgConnection, id: i64) -> ApiResult<bool> {
    let chain = storage::chain(connection, id, false).await?;
    if chain.deleted_at.is_some() {
        return Ok(false);
    }
    if chain.path_kind == "legacy" {
        return Ok(sqlx::query_scalar("SELECT COUNT(*)=2 FROM server_module_status m JOIN servers s ON s.id=m.server_id WHERE m.server_id=ANY(SELECT n.server_id FROM nodes n WHERE n.id=ANY(ARRAY[$1,$2])) AND m.module='singbox' AND m.healthy AND m.applied_rev=m.target_rev AND s.dirty_at IS NULL AND s.deleted_at IS NULL").bind(chain.entry_node_id).bind(chain.exit_node_id).fetch_one(connection).await?);
    }
    if chain.phase != "applied" || !chain.route_enabled || chain.candidate_generation.is_some() {
        return Ok(false);
    }
    let Some(generation) = chain.applied_generation else {
        return Ok(false);
    };
    let version = storage::version(connection, id, generation).await?;
    let servers = BTreeSet::from_iter(std::iter::once(version.snapshot.entry.server_id).chain(
        version.snapshot.hops.iter().filter_map(|hop| match hop {
            FrozenHop::Managed { endpoint, .. } => Some(endpoint.server_id),
            _ => None,
        }),
    ));
    if version.legacy {
        // ADR 0040: a taken-over two-hop generation keeps the two-hop rule. Both
        // servers are healthy, applied and without pending changes; no probe or
        // checkpoint record is required.
        let servers: Vec<i64> = servers.into_iter().collect();
        return Ok(sqlx::query_scalar("SELECT COUNT(*)=$2 FROM server_module_status m JOIN servers s ON s.id=m.server_id WHERE m.server_id=ANY($1) AND m.module='singbox' AND m.healthy AND m.applied_rev=m.target_rev AND s.dirty_at IS NULL AND s.deleted_at IS NULL")
            .bind(&servers).bind(servers.len() as i64).fetch_one(connection).await?);
    }
    let mut observed_vector = Vec::<(i64, Value)>::new();
    for server in servers {
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_path_deployment_dependencies d JOIN server_module_status m ON m.server_id=d.server_id AND m.module=d.module JOIN servers s ON s.id=m.server_id JOIN runtime_module_checkpoints r ON r.server_id=m.server_id AND r.module=m.module JOIN runtime_deployment_bindings b ON b.server_id=d.server_id AND b.module=d.module AND b.rev=d.revision WHERE d.chain_id=$1 AND d.generation=$2 AND d.server_id=$3 AND d.module='singbox' AND d.revision=m.target_rev AND d.revision=m.applied_rev AND m.healthy AND s.dirty_at IS NULL AND s.deleted_at IS NULL AND r.checkpoint_json->>'healthy'='true' AND r.checkpoint_json->'binding'->>'binding_digest'=b.binding_digest AND r.checkpoint_json->'binding'->>'deployment_id'=b.deployment_id::text AND r.checkpoint_json->'binding'->>'bundle_sha256'=b.bundle_sha256 AND (r.checkpoint_json->'binding'->>'revision')::bigint=d.revision AND ($4 OR EXISTS(SELECT 1 FROM singbox_chain_runtime_requirements p JOIN singbox_runtime_manifest_facts f ON f.server_id=p.server_id AND f.module='singbox' AND f.revision=d.revision AND f.artifact_sha256=p.artifact_sha256 AND f.runtime_version=p.runtime_version WHERE p.chain_id=$1 AND p.generation=$2 AND p.server_id=$3)))").bind(id).bind(generation).bind(server).bind(version.legacy).fetch_one(&mut *connection).await?;
        if !valid {
            return Ok(false);
        }
        let observed=sqlx::query_scalar("SELECT checkpoint_json FROM runtime_module_checkpoints WHERE server_id=$1 AND module='singbox'").bind(server).fetch_one(&mut *connection).await?;
        observed_vector.push((server, observed));
    }
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_path_probes p JOIN runtime_control_requests q ON q.request_id=p.request_id JOIN runtime_control_receipts f ON f.request_id=q.request_id JOIN runtime_module_checkpoints r ON r.server_id=p.server_id AND r.module='singbox' WHERE p.chain_id=$1 AND p.generation=$2 AND p.stage='switched' AND p.state='verified' AND p.server_id=$3 AND p.dependency_vector=$4 AND f.outcome='verified' AND q.kind='probe' AND q.request_json->'expected'=r.checkpoint_json AND f.result_json->'observed'=r.checkpoint_json AND f.result_json->>'success'='true' AND f.result_json->>'probe_id'=p.probe_id::text)").bind(id).bind(generation).bind(version.snapshot.entry.server_id).bind(json!(observed_vector)).fetch_one(connection).await?)
}

async fn act(state: &AppState, action: Action) -> ApiResult<()> {
    match action {
        Action::Probe {
            chain_id,
            generation,
            phase: expected_phase,
            stage,
            server,
            probe_id,
            expected,
            vector,
        } => {
            let mut tx = state.pool.begin().await?;
            super::super::entitlements::lock(&mut tx).await?;
            let chain = storage::lifecycle_chain(&mut tx, chain_id, true).await?;
            if chain.phase != expected_phase
                || (matches!(expected_phase.as_str(), "restoring" | "applied")
                    && chain.applied_generation != Some(generation))
                || (!matches!(expected_phase.as_str(), "restoring" | "applied")
                    && chain.candidate_generation != Some(generation))
            {
                tx.commit().await?;
                return Ok(());
            }
            let servers = storage::referenced_servers(&mut tx, chain_id).await?;
            super::super::proxy_resources::lock_cleanup_servers(&mut tx, &servers).await?;
            if current_vector(&mut tx, chain_id, generation)
                .await?
                .as_ref()
                != Some(&vector)
            {
                tx.commit().await?;
                return Ok(());
            }
            let(saved,saved_vector):(Option<Uuid>,Option<Value>)=sqlx::query_as("SELECT request_id,dependency_vector FROM singbox_path_probes WHERE chain_id=$1 AND generation=$2 AND stage=$3 FOR UPDATE").bind(chain_id).bind(generation).bind(&stage).fetch_one(&mut *tx).await?;
            let raw: Option<Value> = if saved_vector.as_ref() == Some(&json!(vector)) {
                sqlx::query_scalar("SELECT request_json FROM runtime_control_requests WHERE request_id=$1 AND server_id=$2 AND module='singbox' AND kind='probe'").bind(saved).bind(server).fetch_optional(&mut *tx).await?
            } else {
                None
            };
            let saved_request = raw
                .map(serde_json::from_value::<RuntimePathProbeRequest>)
                .transpose()
                .map_err(anyhow::Error::from)?;
            let cached = saved_request.filter(|request| {
                Some(request.request_id) == saved
                    && request.expected == expected
                    && request.probe_id == probe_id
                    && request.valid()
            });
            let mut notify = None;
            let request = if let Some(request) = cached {
                request
            } else {
                if let Some(id) = saved {
                    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_control_requests q WHERE q.request_id=$1 AND q.kind='probe' AND q.state='pending' AND q.expires_at>$2 AND NOT EXISTS(SELECT 1 FROM runtime_control_receipts f WHERE f.request_id=q.request_id))").bind(id).bind(now_timestamp()).fetch_one(&mut *tx).await?;
                    if pending {
                        tx.commit().await?;
                        return Ok(());
                    }
                }
                let Ok(request) = runtime_control::enqueue_path_probe(
                    &mut tx,
                    server,
                    "singbox",
                    probe_id,
                    Some(Uuid::new_v4()),
                )
                .await
                else {
                    tx.rollback().await?;
                    return Ok(());
                };
                if request.expected != expected {
                    tx.rollback().await?;
                    return Ok(());
                }
                notify = Some(request.clone());
                request
            };
            let request_id = request.request_id;
            // Queue insertion and the complete guarded-vector association commit
            // together. No convenience replay can adopt an unassociated older request.
            sqlx::query("UPDATE singbox_path_probes SET request_id=$4,revision=$5,state='pending',observed_at=NULL,error=NULL,dependency_vector=$6 WHERE chain_id=$1 AND generation=$2 AND stage=$3 AND request_id IS DISTINCT FROM $4").bind(chain_id).bind(generation).bind(&stage).bind(request_id).bind(expected.binding.revision as i64).bind(json!(vector)).execute(&mut *tx).await?;
            let fact: Option<(String, Value)> = sqlx::query_as(
                "SELECT outcome,result_json FROM runtime_control_receipts WHERE request_id=$1",
            )
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await?;
            let fact = fact
                .map(|(outcome, value)| {
                    serde_json::from_value::<RuntimePathProbeResult>(value)
                        .map(|result| (outcome, result))
                })
                .transpose()
                .map_err(anyhow::Error::from)?;
            let expired:bool=sqlx::query_scalar("SELECT expires_at<=$2 OR state='expired' FROM runtime_control_requests WHERE request_id=$1").bind(request_id).bind(now_timestamp()).fetch_one(&mut *tx).await?;
            if fact
                .as_ref()
                .is_some_and(|(outcome, _)| outcome == "superseded")
            {
                // Another path may dirty the same server after this request was
                // sent. Its terminal receipt is historical proof of the old
                // expectation, not a native path failure or current qualification.
                // Clear only our association and obtain a new opaque request once
                // the complete current vector is ready; never relabel the receipt.
                sqlx::query("UPDATE singbox_path_probes SET request_id=NULL,revision=NULL,state='pending',observed_at=NULL,error='运行配置确认已失效，等待完整当前向量重新探测',dependency_vector=NULL WHERE chain_id=$1 AND generation=$2 AND stage=$3 AND request_id=$4")
                    .bind(chain_id).bind(generation).bind(&stage).bind(request_id).execute(&mut *tx).await?;
                tx.commit().await?;
                return Ok(());
            }
            if let Some((outcome, result)) = fact {
                let verified = outcome == "verified"
                    && result.success
                    && result.probe_id == probe_id
                    && result.observed.as_ref() == Some(&expected);
                sqlx::query("UPDATE singbox_path_probes SET state=$4,observed_at=(SELECT received_at FROM runtime_control_receipts WHERE request_id=$5),error=$6 WHERE chain_id=$1 AND generation=$2 AND stage=$3 AND request_id=$5").bind(chain_id).bind(generation).bind(&stage).bind(if verified{"verified"}else if outcome=="late"{"expired"}else{"failed"}).bind(request_id).bind((!verified).then_some("指定出站路径探测未通过，保留历史成功事实")).execute(&mut *tx).await?;
                if verified {
                    if expected_phase == "restoring"
                        && chain.restore_step.as_deref() == Some("cleanup")
                    {
                        finish_applied(
                            &mut tx,
                            &chain,
                            generation,
                            chain
                                .candidate_generation
                                .unwrap_or(chain.desired_generation),
                            "restoring_cleanup",
                            false,
                        )
                        .await?;
                    } else if expected_phase == "restoring" {
                        sqlx::query("UPDATE singbox_chains SET restore_step='barrier' WHERE id=$1")
                            .bind(chain_id)
                            .execute(&mut *tx)
                            .await?;
                    } else if expected_phase == "retiring_old" {
                        finish_applied(
                            &mut tx,
                            &chain,
                            generation,
                            generation,
                            "retiring_old",
                            true,
                        )
                        .await?;
                    } else if expected_phase == "applied" {
                    } else if stage == "candidate" {
                        // A converting chain hands its entry over here; this is the
                        // last point at which it can return to mixed.
                        if chain.path_kind != "mixed"
                            || crate::mixed_conversion::switch(&mut tx, &chain).await?
                        {
                            probe_row(&mut tx, chain_id, generation, "switched", server).await?;
                            sqlx::query("UPDATE singbox_chains SET route_enabled=TRUE WHERE id=$1")
                                .bind(chain_id)
                                .execute(&mut *tx)
                                .await?;
                            phase(&mut tx, &chain, "switching_entry").await?;
                        }
                    } else {
                        phase(&mut tx, &chain, "fixing_barrier").await?;
                    }
                } else {
                    fail_candidate(&mut tx, &chain, "指定出站路径探测失败，未推进恢复边界").await?;
                }
            } else if expired {
                sqlx::query("UPDATE singbox_path_probes SET state='expired',observed_at=NULL,error='设备路径探测未在期限内确认' WHERE chain_id=$1 AND generation=$2 AND stage=$3 AND request_id=$4").bind(chain_id).bind(generation).bind(&stage).bind(request_id).execute(&mut *tx).await?;
                fail_candidate(&mut tx, &chain, "设备路径探测超时，未把等待确认标为成功").await?;
            }
            tx.commit().await?;
            if let Some(request) = notify {
                runtime_control::notify_path_probe(state, server, &request)
                    .await
                    .map_err(ApiError::Internal)?;
            }
        }
        Action::Barrier {
            chain_id,
            generation,
            phase: expected_phase,
            stage,
            server,
            expected,
            vector,
        } => {
            let mut tx = state.pool.begin().await?;
            super::super::entitlements::lock(&mut tx).await?;
            let chain = storage::lifecycle_chain(&mut tx, chain_id, true).await?;
            if chain.phase != expected_phase
                || chain
                    .candidate_generation
                    .unwrap_or(chain.desired_generation)
                    != generation
            {
                tx.commit().await?;
                return Ok(());
            }
            let servers = storage::referenced_servers(&mut tx, chain_id).await?;
            super::super::proxy_resources::lock_cleanup_servers(&mut tx, &servers).await?;
            let semantic_generation = if expected_phase == "restoring" {
                chain.applied_generation.unwrap_or(generation)
            } else {
                generation
            };
            if current_vector(&mut tx, chain_id, semantic_generation)
                .await?
                .as_ref()
                != Some(&vector)
            {
                tx.commit().await?;
                return Ok(());
            }
            let(saved,saved_vector):(Option<Uuid>,Option<Value>)=sqlx::query_as("SELECT barrier_request_id,barrier_vector FROM singbox_path_stage_deployments WHERE chain_id=$1 AND generation=$2 AND stage=$3 AND server_id=$4 FOR UPDATE").bind(chain_id).bind(generation).bind(&stage).bind(server).fetch_one(&mut *tx).await?;
            if saved.is_some() && saved_vector.as_ref() != Some(&json!(vector)) {
                let receipt:Option<(String,Value,Value,String)>=sqlx::query_as("SELECT f.outcome,f.result_json,q.request_json,q.request_digest FROM runtime_control_requests q JOIN runtime_control_receipts f ON f.request_id=q.request_id WHERE q.request_id=$1 AND q.kind='barrier'").bind(saved).fetch_optional(&mut *tx).await?;
                if let Some((outcome, result, request, digest)) = receipt {
                    let committed = result.get("request_digest").and_then(Value::as_str)
                        == Some(digest.as_str())
                        && result.get("success") == Some(&json!(true))
                        && result.get("pending_intents_clear") == Some(&json!(true))
                        && result.get("observed") == request.get("expected")
                        && request
                            .get("minimum_revision")
                            .and_then(Value::as_u64)
                            .is_some_and(|minimum| {
                                result
                                    .get("minimum_revision")
                                    .and_then(Value::as_u64)
                                    .is_some_and(|floor| floor >= minimum)
                            });
                    if committed && outcome != "verified" && expected_phase != "restoring" {
                        sqlx::query("UPDATE singbox_chains SET minimum_generation=GREATEST(minimum_generation,$2),phase='failed',last_error='设备已提交恢复屏障，但当前完整向量确认已过期；禁止倒回旧路径' WHERE id=$1").bind(chain_id).bind(generation).execute(&mut *tx).await?;
                        super::super::business::mark_dirty(&mut tx, &servers).await?;
                    } else {
                        // The old immutable request/fact remains in runtime history. A
                        // terminal receipt resolves its floor ambiguity, allowing a new
                        // barrier only after a probe confirms the new complete vector.
                        sqlx::query("UPDATE singbox_path_stage_deployments SET barrier_request_id=NULL,barrier_vector=NULL WHERE chain_id=$1 AND generation=$2 AND stage=$3 AND server_id=$4 AND barrier_request_id=$5").bind(chain_id).bind(generation).bind(&stage).bind(server).bind(saved).execute(&mut *tx).await?;
                        if !probe_matches_vector(&mut tx, chain_id, semantic_generation, &vector)
                            .await?
                        {
                            if expected_phase == "restoring" {
                                sqlx::query(
                                    "UPDATE singbox_chains SET restore_step='probe' WHERE id=$1",
                                )
                                .bind(chain_id)
                                .execute(&mut *tx)
                                .await?;
                            } else {
                                phase(&mut tx, &chain, "probing_switched").await?;
                            }
                        }
                    }
                } else {
                    sqlx::query("UPDATE singbox_chains SET last_error='完整路径向量已改变，等待原恢复屏障的设备持久回执后重新确认' WHERE id=$1").bind(chain_id).execute(&mut *tx).await?;
                }
                tx.commit().await?;
                return Ok(());
            }
            // The switched proof must certify this vector before any irreversible
            // command is sent. Checking only the entry activation misses managed restarts.
            let legacy = storage::version(&mut tx, chain_id, semantic_generation)
                .await?
                .legacy;
            if !legacy
                && !probe_matches_vector(&mut tx, chain_id, semantic_generation, &vector).await?
            {
                if expected_phase == "restoring" {
                    sqlx::query("UPDATE singbox_chains SET restore_step='probe' WHERE id=$1")
                        .bind(chain_id)
                        .execute(&mut *tx)
                        .await?;
                } else {
                    phase(&mut tx, &chain, "probing_switched").await?;
                }
                tx.commit().await?;
                return Ok(());
            }
            let raw:Option<Value>=sqlx::query_scalar("SELECT request_json FROM runtime_control_requests WHERE request_id=$1 AND server_id=$2 AND module='singbox' AND kind='barrier'").bind(saved).bind(server).fetch_optional(&mut *tx).await?;
            let cached = raw
                .map(serde_json::from_value::<RuntimeRecoveryBarrierRequest>)
                .transpose()
                .map_err(anyhow::Error::from)?
                .filter(|request| {
                    Some(request.request_id) == saved
                        && request.expected == expected
                        && request.minimum_revision >= expected.binding.revision
                        && request.valid()
                });
            let mut notify = None;
            let request = if let Some(request) = cached {
                request
            } else {
                let Ok(request) = runtime_control::enqueue_barrier(
                    &mut tx,
                    server,
                    "singbox",
                    expected.binding.revision,
                    Some(Uuid::new_v4()),
                )
                .await
                else {
                    tx.rollback().await?;
                    return Ok(());
                };
                if request.expected != expected {
                    tx.rollback().await?;
                    return Ok(());
                }
                notify = Some(request.clone());
                request
            };
            let request_id = request.request_id;
            sqlx::query("UPDATE singbox_path_stage_deployments SET barrier_request_id=$5,barrier_vector=$6 WHERE chain_id=$1 AND generation=$2 AND stage=$3 AND server_id=$4").bind(chain_id).bind(generation).bind(&stage).bind(server).bind(request_id).bind(json!(vector)).execute(&mut *tx).await?;
            let receipt: Option<(String, Value)> = sqlx::query_as(
                "SELECT outcome,result_json FROM runtime_control_receipts WHERE request_id=$1",
            )
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some((outcome, result)) = receipt {
                let committed = result.get("success") == Some(&json!(true))
                    && result.get("pending_intents_clear") == Some(&json!(true))
                    && result.get("observed") == Some(&json!(expected))
                    && result
                        .get("minimum_revision")
                        .and_then(Value::as_u64)
                        .is_some_and(|floor| floor >= expected.binding.revision);
                let valid = outcome == "verified" && committed;
                if valid {
                    if expected_phase == "restoring" {
                        sqlx::query("UPDATE singbox_chains SET restore_step='cleanup' WHERE id=$1")
                            .bind(chain_id)
                            .execute(&mut *tx)
                            .await?;
                        let servers = storage::referenced_servers(&mut tx, chain_id).await?;
                        super::super::business::mark_dirty(&mut tx, &servers).await?;
                    } else {
                        sqlx::query("UPDATE singbox_chains SET minimum_generation=GREATEST(minimum_generation,$2) WHERE id=$1").bind(chain_id).bind(generation).execute(&mut *tx).await?;
                        phase(&mut tx, &chain, "retiring_old").await?;
                    }
                } else if committed && expected_phase != "restoring" {
                    sqlx::query("UPDATE singbox_chains SET minimum_generation=GREATEST(minimum_generation,$2),phase='failed',last_error='设备已提交恢复屏障，但当前完整向量确认已过期；保留新代数，禁止倒回旧路径' WHERE id=$1").bind(chain_id).bind(generation).execute(&mut *tx).await?;
                } else {
                    fail_candidate(&mut tx, &chain, "恢复屏障尚未确认，不撤销可恢复的旧身份")
                        .await?;
                }
            } else {
                let expired:bool=sqlx::query_scalar("SELECT expires_at<=$2 OR state='expired' FROM runtime_control_requests WHERE request_id=$1").bind(request_id).bind(now_timestamp()).fetch_one(&mut *tx).await?;
                // Until an authenticated terminal receipt establishes whether the device
                // committed its floor, retain both generations and keep this durable request.
                if expired {
                    sqlx::query("UPDATE singbox_chains SET last_error='恢复屏障确认超时，等待设备持久回执；尚不清理旧身份或回退路径' WHERE id=$1").bind(chain_id).execute(&mut *tx).await?;
                }
            }
            tx.commit().await?;
            if let Some(request) = notify {
                runtime_control::notify_barrier(state, server, &request)
                    .await
                    .map_err(ApiError::Internal)?;
            }
        }
    }
    Ok(())
}
pub async fn tick(state: &AppState) -> ApiResult<()> {
    capture_committed_floors(state).await?;
    let ids:Vec<i64>=sqlx::query_scalar("SELECT id FROM singbox_chains WHERE (path_kind='ordered' AND (deleted_at IS NULL OR phase<>'retired')) OR id IN (SELECT chain_id FROM singbox_mixed_conversions WHERE state='preparing') ORDER BY id").fetch_all(&state.pool).await?;
    let mut first = None;
    for id in ids {
        match advance(state, id).await {
            Ok(Some(action)) => {
                if let Err(error) = act(state, action).await {
                    first.get_or_insert(error);
                }
            }
            Ok(None) => {}
            Err(error) => {
                first.get_or_insert(error);
            }
        }
    }
    match first {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn capture_committed_floors(state: &AppState) -> ApiResult<()> {
    let mut tx = state.pool.begin().await?;
    super::super::entitlements::lock(&mut tx).await?;
    // Associate committed device floors with immutable published active routes. This also
    // covers a panel crash between durable enqueue and the local phase/request association.
    let floors:Vec<(i64,i64,bool)>=sqlx::query_as("SELECT d.chain_id,MAX(d.generation),BOOL_OR(f.outcome<>'verified') FROM runtime_control_requests q JOIN runtime_control_receipts f ON f.request_id=q.request_id JOIN singbox_path_deployment_dependencies d ON d.server_id=q.server_id AND d.module=q.module AND d.revision=(q.request_json->'expected'->'binding'->>'revision')::bigint JOIN runtime_deployment_bindings b ON b.server_id=d.server_id AND b.module=d.module AND b.rev=d.revision JOIN singbox_chains c ON c.id=d.chain_id WHERE q.kind='barrier' AND d.role='entry' AND d.route_active AND q.request_json->'expected'->'binding'->>'deployment_id'=b.deployment_id::text AND q.request_json->'expected'->'binding'->>'bundle_sha256'=b.bundle_sha256 AND q.request_json->'expected'->'binding'->>'binding_digest'=b.binding_digest AND f.result_json->>'request_digest'=q.request_digest AND f.result_json->'observed'=q.request_json->'expected' AND f.result_json->>'success'='true' AND f.result_json->>'pending_intents_clear'='true' AND (q.request_json->>'minimum_revision')::bigint>=d.revision AND (f.result_json->>'minimum_revision')::bigint>=(q.request_json->>'minimum_revision')::bigint AND c.path_kind='ordered' AND d.generation>c.minimum_generation GROUP BY d.chain_id ORDER BY d.chain_id").fetch_all(&mut *tx).await?;
    for (id, generation, late) in floors {
        let chain = storage::chain(&mut tx, id, true).await?;
        sqlx::query("UPDATE singbox_chains SET minimum_generation=GREATEST(minimum_generation,$2),phase=CASE WHEN $3 OR (phase='restoring' AND applied_generation<$2) THEN 'failed' ELSE phase END,last_error=CASE WHEN $3 OR (phase='restoring' AND applied_generation<$2) THEN '设备已提交恢复屏障，但当前向量确认已过期；禁止倒回较低代数路径' ELSE last_error END WHERE id=$1").bind(id).bind(generation).bind(late).execute(&mut *tx).await?;
        if late || chain.phase == "restoring" {
            let servers = storage::referenced_servers(&mut tx, id).await?;
            super::super::business::mark_dirty(&mut tx, &servers).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

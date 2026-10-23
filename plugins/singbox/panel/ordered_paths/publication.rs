use super::{models::*, storage};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use apsp_compiler::{ManagedAcceptance, Node, OrderedPath, ProbeControl, Relay};
use apsp_protocol::RuntimeBinding;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub(crate) struct Plan {
    pub nodes: Vec<Node>,
    pub paths: Vec<OrderedPath>,
    pub accepts: Vec<ManagedAcceptance>,
    pub legacy: Vec<Relay>,
    pub probe_control: Option<ProbeControl>,
    pub probe_plan: Option<Value>,
    pub dependencies: Vec<SnapshotDependency>,
    pub stages: Vec<(i64, i64, String, String, i32)>,
    pub versioned: bool,
}
fn legacy_relay(id: i64, frozen: &FrozenVersion) -> ApiResult<Relay> {
    let Some(FrozenHop::Managed {
        endpoint,
        relay_uuid,
    }) = frozen.hops.first()
    else {
        return Err(ApiError::Conflict("旧链路没有完整的受管出口快照".into()));
    };
    Ok(Relay {
        settings: endpoint.node.settings.clone(),
        fingerprint: endpoint.node.settings.reality.fingerprint,
        chain_id: id,
        entry_node_id: frozen.entry.node.id,
        exit_node_id: endpoint.node.id,
        uuid: *relay_uuid,
        public_host: endpoint.node.public_host.clone(),
        port: endpoint.node.public_port(),
        sni: endpoint.node.sni.clone(),
        public_key: endpoint.node.public_key.clone(),
        short_id: endpoint.node.short_id.clone(),
    })
}
pub(crate) async fn validate_candidate(
    tx: &mut Transaction<'_, Postgres>,
    path: &OrderedPath,
    frozen: &FrozenVersion,
) -> ApiResult<()> {
    let mut servers = BTreeSet::from([frozen.entry.server_id]);
    let mut accepts = Vec::new();
    for (index, hop) in frozen.hops.iter().enumerate() {
        if let FrozenHop::Managed {
            endpoint,
            relay_uuid,
        } = hop
        {
            servers.insert(endpoint.server_id);
            accepts.push(ManagedAcceptance {
                endpoint: endpoint.as_ref().clone(),
                chain_id: path.chain_id,
                generation: path.generation,
                position: (index + 1) as u8,
                relay_uuid: *relay_uuid,
            });
        }
    }
    for server in servers {
        let query = format!(
            "SELECT {} FROM nodes n WHERE n.server_id=$1 AND n.deleted_at IS NULL ORDER BY n.id",
            super::super::business::NODE_COLUMNS
        );
        let rows = sqlx::query_as::<_, super::super::business::NodeRow>(&query)
            .bind(server)
            .fetch_all(&mut **tx)
            .await?;
        let nodes = rows
            .iter()
            .map(|row| row.model(vec![]))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ApiError::BadRequest("候选服务器含无法解析的节点参数".into()))?;
        let local: Vec<_> = accepts
            .iter()
            .filter(|accept| accept.endpoint.server_id == server)
            .cloned()
            .collect();
        let paths = if server == frozen.entry.server_id {
            vec![path.clone()]
        } else {
            vec![]
        };
        let control = (server == frozen.entry.server_id).then(|| ProbeControl {
            listen_port: 18086,
            secret: "0".repeat(64),
        });
        apsp_compiler::compile_server_with_paths(&nodes, &[], &paths, &local, control.as_ref())
            .map_err(|error| ApiError::BadRequest(format!("候选配置无法编译：{error}")))?;
    }
    Ok(())
}
pub(crate) async fn plan(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    at: i64,
    mut nodes: Vec<Node>,
    mut legacy: Vec<Relay>,
) -> ApiResult<Plan> {
    // A mixed chain under conversion contributes its unrouted candidate only; its
    // entry and users belong to the mixed route until the switch.
    let chains=sqlx::query_as::<_,ChainRow>(&format!("SELECT {CHAIN_COLUMNS} FROM singbox_chains WHERE (path_kind='ordered' AND (deleted_at IS NULL OR phase<>'retired')) OR id IN (SELECT chain_id FROM singbox_mixed_conversions WHERE state='preparing') ORDER BY id")).fetch_all(&mut **tx).await?;
    let mut paths = Vec::new();
    let mut accepts = Vec::new();
    let mut dependencies = Vec::new();
    let mut stages = Vec::new();
    let mut bindings = Vec::new();
    let mut tags = BTreeSet::from(["with_clash_api".to_owned(), "with_v2ray_api".to_owned()]);
    let mut versioned = false;
    for chain in chains {
        let generations = [
            chain.applied_generation,
            chain.candidate_generation,
            chain.recovery_generation,
        ]
        .into_iter()
        .flatten()
        .collect::<BTreeSet<_>>();
        let mut versions = BTreeMap::new();
        for generation in generations {
            versions.insert(
                generation,
                storage::version(tx, chain.id, generation).await?,
            );
        }
        let entry_server = versions
            .values()
            .next()
            .map(|version| version.snapshot.entry.server_id);
        if !versions.values().any(|version|version.snapshot.entry.server_id==server || version.snapshot.hops.iter().any(|hop|matches!(hop,FrozenHop::Managed{endpoint,..}if endpoint.server_id==server))){continue;}
        versioned = true;
        let converting = chain.path_kind == "mixed";
        let live = storage::chain_is_structurally_available(tx, chain.id).await?;
        let granted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM singbox_eligible_accesses($2) WHERE node_id=$1)",
        )
        .bind(chain.entry_node_id)
        .bind(at)
        .fetch_one(&mut **tx)
        .await?;
        if converting && (!live || chain.deleted_at.is_some()) {
            // The lifecycle returns it to mixed; the mixed route retires it.
            continue;
        }
        if !live || chain.deleted_at.is_some() || chain.phase == "retiring" {
            if let Some(node) = nodes.iter_mut().find(|node| node.id == chain.entry_node_id) {
                node.users.clear();
            }
            let position = if entry_server == Some(server) {
                0
            } else {
                versions.values().find_map(|version|version.snapshot.hops.iter().position(|hop|matches!(hop,FrozenHop::Managed{endpoint,..}if endpoint.server_id==server))).map(|position|(position+1)as i32).unwrap_or(1)
            };
            stages.push((
                chain.id,
                chain.desired_generation,
                chain.phase.clone(),
                if entry_server == Some(server) {
                    "entry"
                } else {
                    "managed"
                }
                .into(),
                position,
            ));
            continue;
        }
        if !granted
            && !converting
            && let Some(node) = nodes.iter_mut().find(|node| node.id == chain.entry_node_id)
        {
            node.users.clear();
        }
        let switched = matches!(
            chain.phase.as_str(),
            "switching_entry" | "probing_switched" | "fixing_barrier" | "retiring_old"
        );
        let route_generation = if !chain.route_enabled {
            None
        } else if switched {
            chain.candidate_generation.or(chain.applied_generation)
        } else {
            chain.applied_generation
        };
        let route_generation = route_generation
            .filter(|generation| *generation >= chain.minimum_generation)
            .or_else(|| {
                (chain.route_enabled && chain.phase == "failed")
                    .then_some(chain.candidate_generation)
                    .flatten()
                    .filter(|generation| *generation >= chain.minimum_generation)
            });
        if entry_server == Some(server) && !converting {
            let route_snapshot = route_generation.and_then(|generation| versions.get(&generation));
            if let Some(node) = nodes.iter_mut().find(|node| node.id == chain.entry_node_id) {
                let users = std::mem::take(&mut node.users);
                let name = node.name.clone();
                if let Some(version) = route_snapshot {
                    *node = version.snapshot.entry.node.clone();
                    node.users = users;
                    node.name = name;
                } else {
                    node.users.clear();
                }
            }
        }
        for (generation, version) in &versions {
            let active = route_generation == Some(*generation);
            let candidate = chain.candidate_generation == Some(*generation);
            let entry_candidate = candidate
                && !matches!(chain.phase.as_str(), "preparing_dependencies" | "restoring");
            let retain =
                chain.recovery_generation == Some(*generation) && chain.phase != "retiring_old";
            let include_managed = active
                || (candidate
                    && !(chain.phase == "restoring"
                        && chain.restore_step.as_deref() == Some("cleanup")))
                || retain;
            if entry_server == Some(server) && (active || entry_candidate) {
                if version.legacy {
                    // A legacy relay is compiled only when the old two-hop path would
                    // compile it: both ends live and the entry has an eligible grant.
                    if active && granted {
                        legacy.push(legacy_relay(chain.id, &version.snapshot)?);
                    }
                } else {
                    let path =
                        storage::compiler_path(chain.id, *generation, &version.snapshot, active);
                    tags.extend(apsp_compiler::required_build_tags(&path));
                    let probes:Vec<(Uuid,String)>=sqlx::query_as("SELECT probe_id,stage FROM singbox_path_probes WHERE chain_id=$1 AND generation=$2 ORDER BY stage").bind(chain.id).bind(generation).fetch_all(&mut **tx).await?;
                    for (probe_id, _) in probes {
                        bindings.push(json!({"id":probe_id.to_string(),"selector":path.final_tag(),"target":storage::probe_target(state)?}));
                    }
                    paths.push(path);
                }
                dependencies.push(SnapshotDependency {
                    chain_id: chain.id,
                    generation: *generation,
                    role: "entry".into(),
                    hop_position: 0,
                    route_active: active,
                });
            }
            if include_managed {
                for (index, hop) in version.snapshot.hops.iter().enumerate() {
                    if let FrozenHop::Managed {
                        endpoint,
                        relay_uuid,
                    } = hop
                    {
                        if endpoint.server_id != server {
                            continue;
                        }
                        if version.legacy {
                            if granted && !legacy.iter().any(|relay| relay.chain_id == chain.id) {
                                legacy.push(legacy_relay(chain.id, &version.snapshot)?);
                            }
                        } else {
                            accepts.push(ManagedAcceptance {
                                endpoint: endpoint.as_ref().clone(),
                                chain_id: chain.id,
                                generation: *generation as u64,
                                position: (index + 1) as u8,
                                relay_uuid: *relay_uuid,
                            });
                        }
                        dependencies.push(SnapshotDependency {
                            chain_id: chain.id,
                            generation: *generation,
                            role: "managed".into(),
                            hop_position: (index + 1) as i32,
                            route_active: false,
                        });
                    }
                }
            }
        }
        let cleanup =
            chain.phase == "restoring" && chain.restore_step.as_deref() == Some("cleanup");
        if entry_server == Some(server)
            && matches!(
                chain.phase.as_str(),
                "preparing_entry"
                    | "probing_candidate"
                    | "switching_entry"
                    | "probing_switched"
                    | "fixing_barrier"
                    | "restoring"
                    | "retiring_old"
                    | "applied"
            )
        {
            let stage_generation = if chain.phase == "applied" {
                chain.applied_generation.unwrap_or(chain.desired_generation)
            } else {
                chain
                    .candidate_generation
                    .unwrap_or(chain.desired_generation)
            };
            let stage = match chain.phase.as_str() {
                "probing_candidate" => "preparing_entry",
                "probing_switched" | "fixing_barrier" => "switching_entry",
                _ => chain.phase.as_str(),
            };
            stages.push((
                chain.id,
                stage_generation,
                if cleanup {
                    "restoring_cleanup".into()
                } else {
                    stage.into()
                },
                "entry".into(),
                0,
            ));
        }
        if entry_server != Some(server)
            && (matches!(
                chain.phase.as_str(),
                "preparing_dependencies"
                    | "preparing_entry"
                    | "probing_candidate"
                    | "switching_entry"
                    | "probing_switched"
                    | "fixing_barrier"
                    | "retiring_old"
                    | "applied"
            ) || cleanup)
        {
            let generation = if chain.phase == "applied" {
                chain.applied_generation.unwrap_or(chain.desired_generation)
            } else {
                chain
                    .candidate_generation
                    .unwrap_or(chain.desired_generation)
            };
            let position=versions.get(&generation).and_then(|version|version.snapshot.hops.iter().position(|hop|matches!(hop,FrozenHop::Managed{endpoint,..}if endpoint.server_id==server))).map(|position|(position+1)as i32).unwrap_or(0);
            let stage = if matches!(
                chain.phase.as_str(),
                "preparing_entry"
                    | "probing_candidate"
                    | "switching_entry"
                    | "probing_switched"
                    | "fixing_barrier"
            ) {
                "preparing_dependencies"
            } else {
                chain.phase.as_str()
            };
            stages.push((
                chain.id,
                generation,
                if cleanup {
                    "restoring_cleanup".into()
                } else {
                    stage.into()
                },
                "managed".into(),
                position,
            ));
        }
    }
    let probe_control = if !paths.is_empty() {
        let proposed = super::super::node_protocol::credential(32);
        let secret = format!("{:x}", Sha256::digest(proposed.as_bytes()));
        sqlx::query("INSERT INTO singbox_path_controller_secrets(server_id,secret) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(server).bind(secret).execute(&mut **tx).await?;
        let secret = sqlx::query_scalar(
            "SELECT secret FROM singbox_path_controller_secrets WHERE server_id=$1",
        )
        .bind(server)
        .fetch_one(&mut **tx)
        .await?;
        Some(ProbeControl {
            listen_port: 18086,
            secret,
        })
    } else {
        None
    };
    let probe_plan=probe_control.as_ref().map(|_|json!({"schema":1,"runtime_version":"1.14.2","required_build_tags":tags.into_iter().collect::<Vec<_>>(),"bindings":bindings}));
    if let Some(plan) = &probe_plan
        && (plan["bindings"]
            .as_array()
            .is_none_or(|bindings| bindings.len() > 256)
            || serde_json::to_vec(plan).map_err(anyhow::Error::from)?.len() > 64 * 1024)
    {
        return Err(ApiError::Conflict(
            "入口服务器的签名路径探测计划超过容量上限".into(),
        ));
    }
    Ok(Plan {
        nodes,
        paths,
        accepts,
        legacy,
        probe_control,
        probe_plan,
        dependencies,
        stages,
        versioned,
    })
}
use sha2::{Digest, Sha256};

pub(crate) fn source(plan: &Plan) -> ApiResult<Value> {
    if !plan.versioned {
        return serde_json::to_value(&plan.nodes).map_err(|error| ApiError::Internal(error.into()));
    }
    let accounting_users = plan
        .nodes
        .iter()
        .flat_map(|node| {
            node.users.iter().map(move |access| AccountingUser {
                user_id: access.user_id,
                node_id: node.id,
                stat_name: apsp_compiler::stat_name(access.user_id, node.id),
            })
        })
        .collect();
    serde_json::to_value(ServerSnapshot {
        schema_version: 1,
        public_nodes: plan.nodes.clone(),
        accounting_users,
        path_dependencies: plan.dependencies.clone(),
    })
    .map_err(|error| ApiError::Internal(error.into()))
}
pub(crate) async fn record(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    revision: i64,
    hash: &str,
    plan: &Plan,
) -> ApiResult<()> {
    let proposed = RuntimeBinding::new(
        Uuid::new_v4(),
        "singbox".into(),
        revision as u64,
        hash.into(),
    );
    sqlx::query("INSERT INTO runtime_deployment_bindings(deployment_id,server_id,module,rev,bundle_sha256,binding_digest) VALUES($1,$2,'singbox',$3,$4,$5) ON CONFLICT(server_id,module,rev) DO NOTHING").bind(proposed.deployment_id).bind(server).bind(revision).bind(hash).bind(&proposed.binding_digest).execute(&mut **tx).await?;
    let binding:(Uuid,String)=sqlx::query_as("SELECT deployment_id,binding_digest FROM runtime_deployment_bindings WHERE server_id=$1 AND module='singbox' AND rev=$2").bind(server).bind(revision).fetch_one(&mut **tx).await?;
    for dependency in &plan.dependencies {
        sqlx::query("INSERT INTO singbox_path_deployment_dependencies(server_id,module,revision,chain_id,generation,role,hop_position,route_active) VALUES($1,'singbox',$2,$3,$4,$5,$6,$7) ON CONFLICT DO NOTHING").bind(server).bind(revision).bind(dependency.chain_id).bind(dependency.generation).bind(&dependency.role).bind(dependency.hop_position).bind(dependency.route_active).execute(&mut **tx).await?;
    }
    for (id, generation, stage, role, position) in &plan.stages {
        sqlx::query("INSERT INTO singbox_path_stage_deployments(chain_id,generation,stage,server_id,role,hop_position,revision,bundle_sha256,deployment_id,binding_digest) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(chain_id,generation,stage,server_id) DO UPDATE SET revision=EXCLUDED.revision,bundle_sha256=EXCLUDED.bundle_sha256,deployment_id=EXCLUDED.deployment_id,binding_digest=EXCLUDED.binding_digest,checkpoint_request_id=NULL,barrier_request_id=NULL,barrier_vector=NULL,observed_at=NULL WHERE singbox_path_stage_deployments.revision<>EXCLUDED.revision").bind(id).bind(generation).bind(stage).bind(server).bind(role).bind(position).bind(revision).bind(hash).bind(binding.0).bind(&binding.1).execute(&mut **tx).await?;
    }
    for node in &plan.nodes {
        sqlx::query("INSERT INTO singbox_deployment_public_projection(server_id,module,revision,node_id,public_fields) VALUES($1,'singbox',$2,$3,$4) ON CONFLICT(server_id,module,revision,node_id) DO UPDATE SET public_fields=EXCLUDED.public_fields").bind(server).bind(revision).bind(node.id).bind(json!({"name":node.name,"public_host":node.public_host,"port":node.port,"sni":node.sni,"public_port":node.public_port()})).execute(&mut **tx).await?;
    }
    Ok(())
}

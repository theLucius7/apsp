mod advance;
pub use advance::advance;

use super::models::servers;
use apsp_compiler::{
    Node, Relay,
    paths::{self, Control, Path},
};
use sqlx::{Postgres, Row, Transaction};
use std::collections::{BTreeMap, BTreeSet};

pub struct Prepared {
    pub compiled: paths::Compiled,
    pub evidence: Vec<Evidence>,
}
pub struct Evidence {
    pub chain: i64,
    pub generation: i64,
    pub role: &'static str,
}

pub async fn compile_on(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    nodes: &[Node],
    legacy: &[Relay],
) -> anyhow::Result<Prepared> {
    // A chain under conversion is still compiled here: before the switch it is a
    // mixed chain as usual; after it, only its active generation is kept on the
    // dependencies (see `switched` below). Its managed hops must still be live.
    let rows=sqlx::query("SELECT v.*,c.active_generation,c.pending_generation,c.minimum_generation,c.deleted_at,m.state AS conversion,CASE WHEN c.path_kind='mixed' THEN singbox_path_resources_available(c.id) ELSE NOT EXISTS(SELECT 1 FROM singbox_chain_hops h LEFT JOIN nodes n ON n.id=h.managed_node_id LEFT JOIN servers s ON s.id=h.managed_server_id WHERE h.chain_id=c.id AND h.generation=c.active_generation AND h.kind='managed' AND (n.deleted_at IS NOT NULL OR NOT n.enabled OR s.deleted_at IS NOT NULL)) END AS available FROM singbox_chain_versions v JOIN singbox_chains c ON c.id=v.chain_id LEFT JOIN singbox_mixed_conversions m ON m.chain_id=c.id AND m.state IN ('preparing','switched') WHERE c.path_kind='mixed' OR m.chain_id IS NOT NULL ORDER BY v.chain_id,v.generation").fetch_all(&mut **tx).await?;
    let mut paths = Vec::new();
    let mut blocked = BTreeSet::new();
    let mut retired = BTreeMap::<String, u64>::new();
    let mut evidence = Vec::new();
    let mut entry_active = BTreeSet::new();
    let pending_stages: BTreeMap<i64, String> = rows
        .iter()
        .filter(|r| r.get::<Option<i64>, _>("pending_generation") == Some(r.get("generation")))
        .map(|r| (r.get("chain_id"), r.get("stage")))
        .collect();
    for row in rows {
        let mut path: Path = serde_json::from_value(row.get("path_json"))?;
        if !servers(&path).contains(&server) {
            continue;
        }
        let generation: i64 = row.get("generation");
        let active: Option<i64> = row.get("active_generation");
        let pending: Option<i64> = row.get("pending_generation");
        let stage = pending_stages
            .get(&path.chain_id)
            .map(String::as_str)
            .unwrap_or("active");
        let live =
            row.get::<Option<i64>, _>("deleted_at").is_none() && row.get::<bool, _>("available");
        let candidate = Some(generation) == pending;
        let old = Some(generation) == active;
        let entry = server == path.entry_server_id;
        if row.get::<Option<String>, _>("conversion").as_deref() == Some("switched") {
            // The entry routes the ordered candidate now. Like an ordered recovery
            // generation, the old identities stay on the dependencies until the
            // ordered barrier; every floor of the scope is in the tombstones.
            if !entry && old && live {
                path.active = false;
                evidence.push(Evidence {
                    chain: path.chain_id,
                    generation,
                    role: "dependency",
                });
                paths.push(path);
            }
            continue;
        }
        if entry {
            blocked.insert(path.entry_node_id);
        }
        let switching = matches!(
            stage,
            "switching_entry" | "checking_active" | "establishing_barrier" | "retiring_old"
        );
        let capable: bool = sqlx::query_scalar(
            "SELECT NOT EXISTS(SELECT 1 FROM servers WHERE id=ANY($1) AND NOT capabilities ? $2)",
        )
        .bind(servers(&path))
        .bind(paths::PATH_CAPABILITY)
        .fetch_one(&mut **tx)
        .await?;
        let previously_prepared:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_path_deployments WHERE chain_id=$1 AND generation=$2 AND role<>'retired')").bind(path.chain_id).bind(generation).fetch_one(&mut **tx).await?;
        let candidate_allowed = capable || previously_prepared || stage != "waiting_dependencies";
        let prepared = live
            && ((old && stage != "retiring_old")
                || (candidate
                    && candidate_allowed
                    && !matches!(stage, "failed" | "rolling_back")
                    && (!entry || stage != "waiting_dependencies")));
        if prepared {
            path.active = entry && if switching { candidate } else { old };
            if path.active {
                entry_active.insert(path.entry_node_id);
            }
            let role = if !entry {
                "dependency"
            } else if path.active {
                "active"
            } else {
                "candidate"
            };
            evidence.push(Evidence {
                chain: path.chain_id,
                generation,
                role,
            });
            paths.push(path);
        } else if live && candidate && stage == "rolling_back" && !entry {
            // Retain candidate identities until the entry confirms it no longer
            // uses them. This also permits recovery after a lost failure reply.
            path.active = false;
            evidence.push(Evidence {
                chain: path.chain_id,
                generation,
                role: "dependency",
            });
            paths.push(path);
        } else {
            evidence.push(Evidence {
                chain: path.chain_id,
                generation,
                role: "retired",
            });
            // A partially acknowledged barrier may already exceed the panel's
            // committed floor. Explicit revocation must retire every issued
            // generation, without requiring an unsafe rollback first.
            let floor: u64 = if live {
                row.get::<i64, _>("minimum_generation").try_into()?
            } else {
                path.generation
            };
            if floor > 0 {
                retired
                    .entry(paths::scope(path.chain_id))
                    .and_modify(|v| *v = (*v).max(floor))
                    .or_insert(floor);
            }
        }
    }
    for id in entry_active {
        blocked.remove(&id);
    }
    let tombstones: Vec<(String, i64)> =
        sqlx::query_as("SELECT scope,floor FROM singbox_retired_path_scopes WHERE server_id=$1")
            .bind(server)
            .fetch_all(&mut **tx)
            .await?;
    for (scope, floor) in tombstones {
        let floor: u64 = floor.try_into()?;
        retired
            .entry(scope)
            .and_modify(|value| *value = (*value).max(floor))
            .or_insert(floor);
    }
    let control = sqlx::query_as::<_, (String, String)>(
        "SELECT secret,test_url FROM singbox_path_controls WHERE server_id=$1",
    )
    .bind(server)
    .fetch_optional(&mut **tx)
    .await?
    .map(|(secret, test_url)| Control { secret, test_url });
    let mut compiled = paths::compile(
        server,
        nodes,
        legacy,
        &paths,
        &blocked.into_iter().collect::<Vec<_>>(),
        retired,
        control.as_ref(),
    )?;
    compiled
        .constraints
        .retired
        .retain(|scope, _| !compiled.constraints.active.contains_key(scope));
    Ok(Prepared { compiled, evidence })
}

pub async fn record_deployment_on(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    revision: i64,
    hash: &str,
    evidence: &[Evidence],
) -> anyhow::Result<()> {
    for item in evidence {
        sqlx::query("INSERT INTO singbox_path_deployments(chain_id,generation,server_id,rev,bundle_sha256,role) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
            .bind(item.chain).bind(item.generation).bind(server).bind(revision).bind(hash).bind(item.role).execute(&mut **tx).await?;
    }
    Ok(())
}

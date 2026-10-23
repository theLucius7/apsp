//! Takes legacy two-hop chains over as ordered chains (ADR 0079 phase 3, step
//! S1c). Migration 0044 already froze generation 1 of every two-hop chain with
//! its relay identity; a takeover keeps that generation and changes only the
//! chain's bookkeeping, so devices and users keep receiving the same bytes.
//!
//! Nothing runs on its own: an operator runs `apsp-panel legacy-takeover
//! precheck|apply|rollback`. Every chain has its own transaction. A chain that
//! fails a check, or whose bundles or user subscriptions would change, stays as
//! it was and is reported with its reasons.

use crate::{
    AppState,
    business::{NODE_COLUMNS, NodeRow},
    error::{ApiError, ApiResult},
    ordered_paths::{
        models::{FrozenHop, FrozenVersion},
        storage,
    },
};
use apsp_compiler::Node;
use apsp_protocol::now_timestamp;
use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, Postgres, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Reasons are fixed codes plus public ids; never addresses or credentials.
#[derive(Serialize)]
pub struct ChainOutcome {
    pub chain_id: i64,
    pub outcome: &'static str,
    pub reasons: Vec<String>,
}

#[derive(Default, Serialize)]
pub struct Report {
    pub chains: Vec<ChainOutcome>,
}

impl Report {
    pub fn succeeded(&self) -> bool {
        self.chains.iter().all(|chain| chain.outcome != "skipped")
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Precheck,
    Apply,
    Rollback,
}

#[derive(FromRow)]
struct Chain {
    entry_node_id: i64,
    exit_node_id: Option<i64>,
    relay_uuid: Option<Uuid>,
    path_kind: String,
    phase: String,
    deleted_at: Option<i64>,
    desired_generation: i64,
    applied_generation: Option<i64>,
    candidate_generation: Option<i64>,
    recovery_generation: Option<i64>,
    route_enabled: bool,
}

const CHAIN: &str = "SELECT entry_node_id,exit_node_id,relay_uuid,path_kind,phase,deleted_at,desired_generation,applied_generation,candidate_generation,recovery_generation,route_enabled FROM singbox_chains WHERE id=$1 FOR UPDATE";

/// What a takeover keeps, and what a rollback restores.
struct Target {
    entry: i64,
    exit: i64,
    relay: Uuid,
    servers: [i64; 2],
}

/// Compiled fields of a node; the display name and grants are not frozen.
pub(crate) fn compiled(node: &Node) -> ApiResult<Value> {
    let mut value = serde_json::to_value(node).map_err(anyhow::Error::from)?;
    if let Some(fields) = value.as_object_mut() {
        fields.remove("name");
        fields.remove("users");
    }
    Ok(value)
}

/// Generation 1 must still describe exactly this chain, its relay identity and
/// the current endpoint parameters; the entry may carry no grant that an
/// ordered entry would drop.
async fn generation_one(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    entry: i64,
    exit: i64,
    relay: Uuid,
    reasons: &mut Vec<String>,
) -> ApiResult<Option<[i64; 2]>> {
    let row: Option<(bool, Value)> = sqlx::query_as(
        "SELECT legacy,snapshot FROM singbox_ordered_chain_versions WHERE chain_id=$1 AND generation=1",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((legacy, snapshot)) = row else {
        reasons.push("generation_missing".into());
        return Ok(None);
    };
    let Ok(frozen) = serde_json::from_value::<FrozenVersion>(snapshot) else {
        reasons.push("generation_unreadable".into());
        return Ok(None);
    };
    if !legacy {
        reasons.push("generation_not_legacy".into());
    }
    let hop = match frozen.hops.as_slice() {
        [
            FrozenHop::Managed {
                endpoint,
                relay_uuid,
            },
        ] if endpoint.node.id == exit && *relay_uuid == relay => Some(endpoint.as_ref()),
        _ => None,
    };
    let (hops, matching): (i64, i64) = sqlx::query_as("SELECT COUNT(*),COUNT(*) FILTER (WHERE position=1 AND kind='managed' AND managed_node_id=$2 AND relay_uuid=$3) FROM singbox_ordered_chain_hops WHERE chain_id=$1 AND generation=1")
        .bind(id).bind(exit).bind(relay).fetch_one(&mut **tx).await?;
    let Some(hop) = hop.filter(|_| {
        frozen.entry.node.id == entry
            && frozen.legacy_relay_uuid == Some(relay)
            && hops == 1
            && matching == 1
    }) else {
        reasons.push("generation_mismatch".into());
        return Ok(None);
    };
    for (role, frozen) in [("entry", &frozen.entry), ("exit", hop)] {
        let row: Option<NodeRow> = sqlx::query_as(&format!(
            "SELECT {NODE_COLUMNS} FROM nodes n WHERE n.id=$1 AND n.deleted_at IS NULL"
        ))
        .bind(frozen.node.id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(row) = row else {
            reasons.push(format!("{role}_deleted"));
            continue;
        };
        if row.server_id != frozen.server_id {
            reasons.push(format!("{role}_moved"));
        }
        if row.protocol != "vless-reality" {
            reasons.push(format!("{role}_not_reality"));
        }
        if !row.enabled {
            reasons.push(format!("{role}_disabled"));
        }
        let unchanged = match row.model(vec![]) {
            Ok(node) => Some(compiled(&node)? == compiled(&frozen.node)?),
            Err(_) => None,
        };
        match unchanged {
            Some(true) => {}
            Some(false) => reasons.push(format!("{role}_changed")),
            None => reasons.push(format!("{role}_unreadable")),
        }
    }
    if !storage::chain_is_structurally_available(tx, id).await? {
        reasons.push("not_available".into());
    }
    let (direct, policy): (bool, bool) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM accesses WHERE node_id=$1 AND direct_grant),EXISTS(SELECT 1 FROM singbox_policy_nodes WHERE node_id=$1)")
        .bind(entry).fetch_one(&mut **tx).await?;
    if direct {
        reasons.push("entry_direct_grants".into());
    }
    if policy {
        reasons.push("entry_policy_node_grants".into());
    }
    Ok(reasons
        .is_empty()
        .then_some([frozen.entry.server_id, hop.server_id]))
}

async fn takeover_target(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    chain: &Chain,
    reasons: &mut Vec<String>,
) -> ApiResult<Option<Target>> {
    let (Some(exit), Some(relay)) = (chain.exit_node_id, chain.relay_uuid) else {
        reasons.push("not_legacy".into());
        return Ok(None);
    };
    if chain.path_kind != "legacy" {
        reasons.push("not_legacy".into());
        return Ok(None);
    }
    if chain.deleted_at.is_some() {
        reasons.push("deleted".into());
        return Ok(None);
    }
    if chain.phase != "legacy"
        || chain.desired_generation != 1
        || chain.applied_generation != Some(1)
        || chain.candidate_generation.is_some()
        || chain.recovery_generation.is_some()
    {
        reasons.push("generation_state_changed".into());
        return Ok(None);
    }
    let servers = generation_one(tx, id, chain.entry_node_id, exit, relay, reasons).await?;
    Ok(servers.map(|servers| Target {
        entry: chain.entry_node_id,
        exit,
        relay,
        servers,
    }))
}

/// A taken-over chain returns only while it still runs the taken-over generation.
async fn rollback_target(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    chain: &Chain,
    reasons: &mut Vec<String>,
) -> ApiResult<Option<Target>> {
    let saved: Option<(i64, Uuid)> = sqlx::query_as(
        "SELECT exit_node_id,relay_uuid FROM singbox_legacy_takeovers WHERE chain_id=$1",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((exit, relay)) = saved else {
        reasons.push("not_taken_over".into());
        return Ok(None);
    };
    if chain.path_kind != "ordered"
        || chain.deleted_at.is_some()
        || chain.phase != "applied"
        || !chain.route_enabled
        || chain.desired_generation != 1
        || chain.applied_generation != Some(1)
        || chain.candidate_generation.is_some()
        || chain.recovery_generation.is_some()
    {
        reasons.push("changed_since_takeover".into());
        return Ok(None);
    }
    let servers = generation_one(tx, id, chain.entry_node_id, exit, relay, reasons).await?;
    Ok(servers.map(|servers| Target {
        entry: chain.entry_node_id,
        exit,
        relay,
        servers,
    }))
}

/// Bundle digests of both servers and subscription digests of every user who
/// may receive the entry, computed inside the transaction.
async fn digests(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    target: &Target,
    at: i64,
) -> ApiResult<BTreeMap<String, String>> {
    let mut digests = BTreeMap::new();
    for server in target.servers {
        let digest = crate::publisher::bundle_digest_on(state, tx, server, at).await?;
        digests.insert(
            format!("bundle/{server}"),
            digest.unwrap_or_else(|| "not_enabled".into()),
        );
    }
    let users: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM singbox_desired_accesses WHERE node_id=$1 UNION SELECT user_id FROM accesses WHERE node_id=$1 ORDER BY 1")
        .bind(target.entry).fetch_all(&mut **tx).await?;
    for user in users {
        for format in ["singbox", "links"] {
            let mut savepoint = sqlx::Acquire::begin(&mut **tx).await?;
            let digest = crate::subscriptions::content_digest(&mut savepoint, user, format).await?;
            savepoint.rollback().await?;
            digests.insert(format!("subscription/{user}/{format}"), digest);
        }
    }
    Ok(digests)
}

async fn take_over(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    target: &Target,
    at: i64,
) -> ApiResult<()> {
    let granted: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_eligible_accesses($2) WHERE node_id=$1)",
    )
    .bind(target.entry)
    .bind(at)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("INSERT INTO singbox_legacy_takeovers(chain_id,exit_node_id,relay_uuid,taken_over_at) VALUES($1,$2,$3,$4)")
        .bind(id).bind(target.exit).bind(target.relay).bind(at).execute(&mut **tx).await?;
    // Generation 1 becomes the applied ordered generation as it is; no new
    // generation, relay identity or tag is created.
    sqlx::query("UPDATE singbox_chains SET path_kind='ordered',phase='applied',desired_generation=1,applied_generation=1,candidate_generation=NULL,recovery_generation=NULL,route_enabled=TRUE,restore_step=NULL,last_error=NULL,last_granted=$2,exit_node_id=NULL,relay_uuid=NULL WHERE id=$1")
        .bind(id).bind(granted).execute(&mut **tx).await?;
    Ok(())
}

async fn restore(tx: &mut Transaction<'_, Postgres>, id: i64, target: &Target) -> ApiResult<()> {
    sqlx::query("UPDATE singbox_chains SET path_kind='legacy',phase='legacy',exit_node_id=$2,relay_uuid=$3,route_enabled=TRUE,last_granted=FALSE,last_error=NULL WHERE id=$1")
        .bind(id).bind(target.exit).bind(target.relay).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM singbox_legacy_takeovers WHERE chain_id=$1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn outcome(chain_id: i64, outcome: &'static str, reasons: Vec<String>) -> ChainOutcome {
    ChainOutcome {
        chain_id,
        outcome,
        reasons,
    }
}

async fn one(state: &AppState, id: i64, mode: Mode) -> ApiResult<ChainOutcome> {
    let mut tx = state.pool.begin().await?;
    // Publication reads under the same isolation and lock; see `publisher`.
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    super::entitlements::lock(&mut tx).await?;
    let at = now_timestamp();
    let Some(chain) = sqlx::query_as::<_, Chain>(CHAIN)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
    else {
        return Ok(outcome(id, "skipped", vec!["not_found".into()]));
    };
    let mut reasons = Vec::new();
    let target = if mode == Mode::Rollback {
        rollback_target(&mut tx, id, &chain, &mut reasons).await?
    } else {
        takeover_target(&mut tx, id, &chain, &mut reasons).await?
    };
    let Some(target) = target else {
        tx.rollback().await?;
        return Ok(outcome(id, "skipped", reasons));
    };
    let before = digests(state, &mut tx, &target, at).await?;
    if mode == Mode::Rollback {
        restore(&mut tx, id, &target).await?;
    } else {
        take_over(&mut tx, id, &target, at).await?;
    }
    let after = digests(state, &mut tx, &target, at).await?;
    if before != after {
        tx.rollback().await?;
        let mut reasons = vec!["bytes_changed".to_owned()];
        reasons.extend(
            before
                .keys()
                .chain(after.keys())
                .filter(|key| before.get(*key) != after.get(*key))
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
        );
        return Ok(outcome(id, "skipped", reasons));
    }
    Ok(match mode {
        Mode::Precheck => {
            tx.rollback().await?;
            outcome(id, "ready", vec![])
        }
        Mode::Apply => {
            tx.commit().await?;
            outcome(id, "taken_over", vec![])
        }
        Mode::Rollback => {
            tx.commit().await?;
            outcome(id, "rolled_back", vec![])
        }
    })
}

async fn run(state: &AppState, chains: Option<Vec<i64>>, mode: Mode) -> ApiResult<Report> {
    let ids = match chains {
        Some(ids) => ids,
        None if mode == Mode::Rollback => sqlx::query_scalar("SELECT chain_id FROM singbox_legacy_takeovers ORDER BY chain_id")
            .fetch_all(&state.pool).await?,
        None => sqlx::query_scalar("SELECT id FROM singbox_chains WHERE path_kind='legacy' AND deleted_at IS NULL ORDER BY id")
            .fetch_all(&state.pool).await?,
    };
    if ids.iter().any(|id| *id <= 0) {
        return Err(ApiError::BadRequest("链路编号必须为正整数".into()));
    }
    let mut report = Report::default();
    for id in ids {
        report.chains.push(one(state, id, mode).await?);
    }
    Ok(report)
}

/// Runs every check and the byte comparison, then rolls back.
pub async fn precheck(state: &AppState, chains: Option<Vec<i64>>) -> ApiResult<Report> {
    run(state, chains, Mode::Precheck).await
}

/// Takes over every chain that passes; the others stay legacy.
pub async fn apply(state: &AppState, chains: Option<Vec<i64>>) -> ApiResult<Report> {
    run(state, chains, Mode::Apply).await
}

/// Restores taken-over chains that still run the taken-over generation.
pub async fn rollback(state: &AppState, chains: Option<Vec<i64>>) -> ApiResult<Report> {
    run(state, chains, Mode::Rollback).await
}

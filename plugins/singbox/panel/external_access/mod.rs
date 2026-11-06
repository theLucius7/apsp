//! External nodes reach proxy users only through policy groups (ADR 0112). A
//! group binds a node either to its current version or to one pinned version;
//! subscriptions resolve the binding when they are generated.
pub mod model;
pub(super) use model::subscription_nodes;

use crate::{
    AppState,
    auth::require_admin,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use model::{Binding, Entry, Reference};
use serde::Serialize;
use sqlx::{PgConnection, Postgres, Transaction};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize)]
pub struct Candidates {
    available_nodes: Vec<Entry>,
}

/// Adopted external nodes a group can take now, each bound to its current version.
pub async fn candidates(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    require_admin(&state, &headers).await?;
    let mut tx = crate::read_snapshot(&state.pool).await?;
    let states = model::states(&mut tx, None).await?;
    let versions = model::versions(
        &mut tx,
        states
            .values()
            .filter_map(|node| node.current_version_id)
            .collect(),
    )
    .await?;
    let mut available_nodes = Vec::new();
    for state in states
        .values()
        .filter(|state| state.adopted && !state.deleted && !state.source_deleted)
    {
        if let Some(version_id) = state.current_version_id {
            let reference = Reference {
                external_node_id: state.id,
                source_id: state.source_id,
                identity_epoch: state.identity_epoch,
                node_version_id: version_id,
                update_mode: model::UpdateMode::FollowNode,
                metadata_revision: state.metadata_revision,
            };
            available_nodes.push(state.entry(reference, versions.get(&version_id), true));
        }
    }
    available_nodes.sort_by_key(|entry| {
        (
            states[&entry.reference.external_node_id].sort_order,
            entry.reference.external_node_id,
        )
    });
    tx.commit().await?;
    Ok((
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Json(Candidates { available_nodes }),
    )
        .into_response())
}

fn describe(
    bindings: Vec<Binding>,
    states: &BTreeMap<i64, model::NodeState>,
    versions: &BTreeMap<i64, model::Version>,
) -> Vec<Entry> {
    let mut entries: Vec<Entry> = bindings
        .into_iter()
        .filter_map(|binding| {
            let state = states.get(&binding.external_node_id)?;
            let version = if binding.update_mode == "pinned" {
                versions.get(&binding.node_version_id)
            } else {
                state.current_version_id.and_then(|id| versions.get(&id))
            };
            Some(state.entry(binding.reference(state.metadata_revision), version, false))
        })
        .collect();
    entries.sort_by_key(|entry| {
        (
            states[&entry.reference.external_node_id].sort_order,
            entry.reference.external_node_id,
        )
    });
    entries
}

/// Every group's external nodes, keyed by group id.
pub(crate) async fn group_entries(
    connection: &mut PgConnection,
    group: Option<i64>,
) -> ApiResult<BTreeMap<i64, Vec<Entry>>> {
    let rows = model::group_bindings(connection, group).await?;
    let all: Vec<Binding> = rows
        .iter()
        .map(|(_, binding)| Binding {
            external_node_id: binding.external_node_id,
            source_id: binding.source_id,
            identity_epoch: binding.identity_epoch,
            node_version_id: binding.node_version_id,
            update_mode: binding.update_mode.clone(),
            created_at: binding.created_at,
        })
        .collect();
    let (states, versions) = model::resolve(connection, &all).await?;
    let mut grouped: BTreeMap<i64, Vec<Binding>> = BTreeMap::new();
    for (group, binding) in rows {
        grouped.entry(group).or_default().push(binding);
    }
    Ok(grouped
        .into_iter()
        .map(|(group, bindings)| (group, describe(bindings, &states, &versions)))
        .collect())
}

pub(crate) fn validate(references: &[Reference]) -> ApiResult<Vec<i64>> {
    if references.len() > 5000 {
        return Err(ApiError::BadRequest("外部节点超过 5000 个".into()));
    }
    let mut ids = BTreeSet::new();
    for entry in references {
        if entry.external_node_id <= 0
            || entry.source_id <= 0
            || entry.identity_epoch <= 0
            || entry.node_version_id <= 0
            || entry.external_node_id > super::business::MAX_SAFE_INTEGER
            || entry.source_id > super::business::MAX_SAFE_INTEGER
            || entry.identity_epoch > super::business::MAX_SAFE_INTEGER
            || entry.node_version_id > super::business::MAX_SAFE_INTEGER
            || !(0..=super::business::MAX_SAFE_INTEGER).contains(&entry.metadata_revision)
            || !ids.insert(entry.external_node_id)
        {
            return Err(ApiError::BadRequest("外部节点包含无效或重复的身份".into()));
        }
    }
    Ok(ids.into_iter().collect())
}

/// Replaces a group's external nodes. A node that stays in the group keeps its
/// binding even when it is unavailable now; a node added to it must be adopted,
/// enabled and on its current version.
pub(crate) async fn replace(
    tx: &mut Transaction<'_, Postgres>,
    group: i64,
    references: &[Reference],
) -> ApiResult<()> {
    let ids = validate(references)?;
    let previous: Vec<Binding> = model::group_bindings(tx, Some(group))
        .await?
        .into_iter()
        .map(|(_, binding)| binding)
        .collect();
    let states = model::states(tx, Some(&ids)).await?;
    let versions = model::versions(
        tx,
        references
            .iter()
            .map(|entry| entry.node_version_id)
            .collect(),
    )
    .await?;
    for entry in references {
        let Some(state) = states.get(&entry.external_node_id) else {
            return Err(ApiError::Conflict("所选外部节点已不存在".into()));
        };
        if state.metadata_revision != entry.metadata_revision {
            return Err(ApiError::Conflict(
                "所选外部节点管理设置已变化，请刷新确认".into(),
            ));
        }
        if previous.iter().any(|binding| binding.matches(entry)) {
            continue;
        }
        if state.source_id != entry.source_id
            || state.current_version_id != Some(entry.node_version_id)
            || state
                .reason(
                    versions.get(&entry.node_version_id),
                    entry.identity_epoch,
                    true,
                )
                .is_some()
        {
            return Err(ApiError::Conflict(
                "所选外部节点未采用、已停用或版本与来源身份已变化，请刷新后重新选择".into(),
            ));
        }
    }
    sqlx::query("DELETE FROM singbox_policy_external_nodes WHERE group_id=$1")
        .bind(group)
        .execute(&mut **tx)
        .await?;
    for entry in references {
        let created_at = previous
            .iter()
            .find(|binding| binding.matches(entry))
            .map_or_else(apsp_protocol::now_timestamp, |binding| binding.created_at);
        sqlx::query("INSERT INTO singbox_policy_external_nodes(group_id,external_node_id,identity_epoch,node_version_id,update_mode,created_at) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(group).bind(entry.external_node_id).bind(entry.identity_epoch).bind(entry.node_version_id).bind(entry.update_mode.as_str()).bind(created_at).execute(&mut **tx).await?;
    }
    Ok(())
}

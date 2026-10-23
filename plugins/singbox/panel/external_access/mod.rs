pub(super) mod model;
pub(super) use model::subscription_nodes;

use crate::{
    AppState,
    auth::require_admin,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use model::{Entry, Reference};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use std::collections::BTreeSet;

#[derive(Serialize)]
pub struct AccessView {
    revision: i64,
    accesses: Vec<Entry>,
    available_nodes: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceAccesses {
    revision: i64,
    accesses: Vec<Reference>,
}

async fn view(connection: &mut PgConnection, user_id: i64) -> ApiResult<AccessView> {
    let revision = model::revision(connection, user_id).await?;
    let states = model::states(connection, None).await?;
    let versions = model::versions(
        connection,
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
    let accesses = model::subscription_nodes(connection, user_id)
        .await?
        .entries;
    Ok(AccessView {
        revision,
        accesses,
        available_nodes,
    })
}

fn response(value: AccessView) -> Response {
    (
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Json(value),
    )
        .into_response()
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND deleted_at IS NULL)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    let value = view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(response(value))
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    request: Result<Json<ReplaceAccesses>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<Response> {
    require_admin(&state, &headers).await?;
    let Json(request) =
        request.map_err(|_| ApiError::BadRequest("外部节点授权字段或大小不符合要求".into()))?;
    if !(0..=super::business::MAX_SAFE_INTEGER).contains(&request.revision)
        || request.accesses.len() > 5000
    {
        return Err(ApiError::BadRequest(
            "授权修订号无效或节点超过 5000 个".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    for entry in &request.accesses {
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
            return Err(ApiError::BadRequest(
                "外部节点授权包含无效或重复的身份".into(),
            ));
        }
    }
    let mut tx = state.pool.begin().await?;
    super::entitlements::lock(&mut tx).await?;
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE id=$1 AND deleted_at IS NULL FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }
    let revision = model::revision(&mut tx, id).await?;
    if revision != request.revision {
        return Err(ApiError::Conflict(
            "外部节点授权已变化，请刷新确认；草稿仍保留".into(),
        ));
    }
    let previous = model::bindings(&mut tx, id).await?;
    let states = model::states(&mut tx, Some(&ids.into_iter().collect::<Vec<_>>())).await?;
    let versions = model::versions(
        &mut tx,
        request
            .accesses
            .iter()
            .map(|entry| entry.node_version_id)
            .collect(),
    )
    .await?;
    for entry in &request.accesses {
        let Some(state) = states.get(&entry.external_node_id) else {
            return Err(ApiError::Conflict("所选外部节点已不存在".into()));
        };
        if state.metadata_revision != entry.metadata_revision {
            return Err(ApiError::Conflict(
                "所选外部节点管理设置已变化，请刷新确认".into(),
            ));
        }
        // Preserve an existing inactive binding only when its entire identity and mode match.
        // It stays visible for explicit removal and never bypasses subscription eligibility.
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
                "所选节点未采用、已停用或版本与来源身份已变化，请刷新后重新选择".into(),
            ));
        }
    }
    let next_revision = revision
        .checked_add(1)
        .filter(|next| *next <= super::business::MAX_SAFE_INTEGER)
        .ok_or_else(|| ApiError::Conflict("授权修订号已达上限".into()))?;
    sqlx::query("DELETE FROM singbox_external_accesses WHERE user_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for entry in request.accesses {
        let created_at = previous
            .iter()
            .find(|binding| binding.matches(&entry))
            .map_or_else(apsp_protocol::now_timestamp, |binding| binding.created_at);
        sqlx::query("INSERT INTO singbox_external_accesses(user_id,external_node_id,source_id,identity_epoch,node_version_id,update_mode,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(id).bind(entry.external_node_id).bind(entry.source_id).bind(entry.identity_epoch).bind(entry.node_version_id).bind(entry.update_mode.as_str()).bind(created_at).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO singbox_external_access_state(user_id,revision) VALUES($1,$2) ON CONFLICT(user_id) DO UPDATE SET revision=EXCLUDED.revision")
        .bind(id).bind(next_revision).execute(&mut *tx).await?;
    let value = view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(response(value))
}

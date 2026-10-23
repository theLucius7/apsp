use super::models::*;
use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
use apsp_compiler::paths::{Hop, Path};
use axum::{
    Json,
    extract::{Path as Id, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyVersions {
    pub expected_generation: i64,
    pub versions: Vec<VersionSelection>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionSelection {
    pub position: usize,
    pub node_version_id: i64,
}

pub async fn apply_versions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Id(id): Id<i64>,
    Json(request): Json<ApplyVersions>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    auth::require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    super::super::entitlements::lock(&mut tx).await?;
    let converting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_mixed_conversions WHERE chain_id=$1 AND state='preparing')")
        .bind(id).fetch_one(&mut *tx).await?;
    if converting {
        return Err(ApiError::Conflict(
            "链路正在转换为有序链路，请等待转换结束".into(),
        ));
    }
    let (path, inputs, active, stage) = base_on(&mut tx, id, request.expected_generation).await?;
    if stage != "active" && stage != "failed" {
        return Err(ApiError::Conflict(
            "此链路正在发布或回退，请等待当前操作结束".into(),
        ));
    }
    if request.versions.is_empty() && stage != "failed" {
        return Err(ApiError::BadRequest("请选择要应用的节点版本".into()));
    }
    let next = queue_on(&mut tx, path, inputs, active, &request.versions).await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"generation":next,"stage":"waiting_dependencies"})),
    ))
}

async fn base_on(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    expected: i64,
) -> ApiResult<(Path, Vec<HopInput>, Option<i64>, String)> {
    let row=sqlx::query("SELECT c.active_generation,c.pending_generation,v.path_json,v.stage,v.generation FROM singbox_live_chains c JOIN singbox_chain_versions v ON v.chain_id=c.id AND v.generation=COALESCE(c.pending_generation,c.active_generation) WHERE c.id=$1 AND c.path_kind='mixed' FOR UPDATE OF c").bind(id).fetch_optional(&mut **tx).await?.ok_or(ApiError::NotFound)?;
    let generation: i64 = row.get("generation");
    if generation != expected {
        return Err(ApiError::Conflict(
            "链路版本已经变化，请刷新后重新选择".into(),
        ));
    }
    let path: Path = serde_json::from_value(row.get("path_json")).map_err(anyhow::Error::from)?;
    let inputs = hop_inputs_on(tx, id, generation).await?;
    Ok((path, inputs, row.get("active_generation"), row.get("stage")))
}

async fn queue_on(
    tx: &mut Transaction<'_, Postgres>,
    mut path: Path,
    mut inputs: Vec<HopInput>,
    active: Option<i64>,
    selections: &[VersionSelection],
) -> ApiResult<i64> {
    let mut positions = BTreeSet::new();
    for selection in selections {
        if !positions.insert(selection.position) {
            return Err(ApiError::BadRequest("不能重复选择同一段的版本".into()));
        }
        let Some(HopInput::Subscription {
            source_id,
            external_node_id,
            node_version_id,
            ..
        }) = inputs.get_mut(selection.position)
        else {
            return Err(ApiError::BadRequest(
                "只能为订阅节点选择新的参数版本".into(),
            ));
        };
        let new = super::super::sources::load_version_on(
            tx,
            *source_id,
            *external_node_id,
            selection.node_version_id,
            true,
        )
        .await?;
        *node_version_id = selection.node_version_id;
        path.hops[selection.position] = Hop::External {
            node_id: *external_node_id,
            version_id: *node_version_id,
            outbound: new.outbound,
        };
    }
    for server in servers(&path) {
        super::super::business::lock_server(tx, server).await?;
        super::super::settings::require_enabled(tx, server).await?;
    }
    // Rotate only internal credentials. Listening endpoints and unchanged imported
    // versions remain the exact immutable vector selected for this path.
    for hop in &mut path.hops {
        if let Hop::Managed { identity, .. } = hop {
            *identity = Uuid::new_v4();
        }
    }
    let generation: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(generation),0)+1 FROM singbox_chain_versions WHERE chain_id=$1",
    )
    .bind(path.chain_id)
    .fetch_one(&mut **tx)
    .await?;
    path.generation = generation.try_into().map_err(anyhow::Error::from)?;
    path.active = false;
    validate_frozen_on(tx, &[path.clone()]).await?;
    insert_version_on(tx, &path, &inputs, active).await?;
    super::super::business::mark_dirty(tx, &servers(&path)).await?;
    Ok(generation)
}

/// Poll stable identities only. Absence, ambiguity and a new identity epoch never
/// replace a working path with another node having a similar display name.
pub async fn follow_updates(state: &AppState) -> ApiResult<()> {
    let mut tx = state.pool.begin().await?;
    super::super::entitlements::lock(&mut tx).await?;
    let ids=sqlx::query_as::<_,(i64,i64)>("SELECT id,active_generation FROM singbox_live_chains c WHERE path_kind='mixed' AND active_generation IS NOT NULL AND pending_generation IS NULL AND NOT EXISTS(SELECT 1 FROM singbox_mixed_conversions m WHERE m.chain_id=c.id AND m.state='preparing') ORDER BY id").fetch_all(&mut *tx).await?;
    for (id, generation) in ids {
        let (path, inputs, active, _) = base_on(&mut tx, id, generation).await?;
        let mut updates = Vec::new();
        for (position, input) in inputs.iter().enumerate() {
            if let HopInput::Subscription {
                source_id,
                external_node_id,
                node_version_id,
                update_mode: UpdateMode::FollowNode,
            } = input
            {
                let previous = super::super::sources::load_version_on(
                    &mut tx,
                    *source_id,
                    *external_node_id,
                    *node_version_id,
                    false,
                )
                .await?;
                if let Some(next) = super::super::sources::latest_follow_version_on(
                    &mut tx,
                    *source_id,
                    *external_node_id,
                    previous.identity_epoch,
                )
                .await?
                    && next.config_sha256 != previous.config_sha256
                {
                    updates.push(VersionSelection {
                        position,
                        node_version_id: next.node_version_id,
                    });
                }
            }
        }
        if !updates.is_empty() {
            sqlx::query("SAVEPOINT path_source_update")
                .execute(&mut *tx)
                .await?;
            if queue_on(&mut tx, path, inputs, active, &updates)
                .await
                .is_err()
            {
                sqlx::query("ROLLBACK TO SAVEPOINT path_source_update")
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE singbox_chain_versions SET last_error='来源参数更新与现有链路不兼容，已保留原版本；请检查节点版本' WHERE chain_id=$1 AND generation=$2 AND last_error IS DISTINCT FROM '来源参数更新与现有链路不兼容，已保留原版本；请检查节点版本'").bind(id).bind(generation).execute(&mut *tx).await?;
            }
            sqlx::query("RELEASE SAVEPOINT path_source_update")
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

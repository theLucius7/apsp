//! Configuration versions a device should run and the revisions it reports applying.
//!
//! The device channel hands apply results and applied-revision hints here. The traffic
//! runtime is the only module a device runs today; its manifest entry and its
//! configuration bundles come from `agent`.

use std::collections::BTreeMap;

use apsp_protocol::{
    AppliedRevisions, ApplyResult, ApplyStatus, Envelope, Manifest, ManifestChanged,
    ModuleManifest, now_timestamp,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use sqlx::Row;

use crate::{AppState, agent_api, artifacts, auth, error::ApiResult};

/// The modules a device must run, keyed by module identifier.
pub async fn manifest_modules(
    state: &AppState,
    server_id: i64,
    info: &serde_json::Value,
) -> ApiResult<BTreeMap<String, ModuleManifest>> {
    let mut modules = BTreeMap::new();
    if let Some(module) = crate::agent::manifest_module(state, server_id, info).await? {
        modules.insert("singbox".into(), module);
    }
    Ok(modules)
}

/// The published configuration bundle at one revision.
pub async fn bundle_text(state: &AppState, server_id: i64, rev: i64) -> ApiResult<String> {
    crate::agent::bundle(state, server_id, rev).await
}

pub async fn manifest(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Manifest>> {
    let server_id = auth::require_agent(&state, &headers).await?;
    artifacts::require_signed_agent(&state, server_id).await?;
    let row = sqlx::query("SELECT manifest_rev,static_info FROM servers WHERE id=$1")
        .bind(server_id)
        .fetch_one(&state.pool)
        .await?;
    let rev: i64 = row.get("manifest_rev");
    let info: serde_json::Value = row.get("static_info");
    let modules = manifest_modules(&state, server_id, &info).await?;
    Ok(Json(Manifest {
        rev: rev as u64,
        modules,
    }))
}

pub async fn bundle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rev): Path<i64>,
) -> ApiResult<Response> {
    let server_id = auth::require_agent(&state, &headers).await?;
    let bundle = bundle_text(&state, server_id, rev).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bundle,
    )
        .into_response())
}

/// Compares the revisions a device reports as applied with the recorded state.
pub async fn reconcile_applied(
    state: &AppState,
    server_id: i64,
    applied: AppliedRevisions,
) -> anyhow::Result<()> {
    let mut tx = state.pool.begin().await?;
    crate::runtime_control::lock_server(&mut tx, server_id).await?;
    let exact: bool = sqlx::query_scalar("SELECT COALESCE((SELECT checkpoint_required FROM runtime_control_devices WHERE server_id=$1),false)")
        .bind(server_id).fetch_one(&mut *tx).await?;
    let rows = sqlx::query(
        "SELECT module,target_rev,applied_rev,healthy FROM server_module_status WHERE server_id=$1 FOR UPDATE",
    )
    .bind(server_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut differs = false;
    let mut checks = Vec::new();
    for row in rows {
        let module: String = row.get("module");
        let reported = applied.get(&module).copied().unwrap_or(0);
        let known = row.get::<i64, _>("applied_rev") as u64;
        differs |= reported != row.get::<i64, _>("target_rev") as u64 || reported != known;
        if exact && reported != row.get::<i64, _>("target_rev") as u64 {
            sqlx::query("UPDATE server_module_status SET healthy=false,updated_at=$3 WHERE server_id=$1 AND module=$2")
                .bind(server_id).bind(&module).bind(now_timestamp()).execute(&mut *tx).await?;
        }
        if exact
            && reported > 0
            && reported == row.get::<i64, _>("target_rev") as u64
            && (!row.get::<bool, _>("healthy") || known != reported)
        {
            checks.push(module.clone());
        }
        if !exact
            && reported > known
            && let Ok(rev) = i64::try_from(reported)
        {
            let published: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deployments WHERE server_id=$1 AND module=$2 AND rev=$3)")
                .bind(server_id).bind(&module).bind(rev).fetch_one(&mut *tx).await?;
            if published {
                // Applied checkpoints are durable on the device even if their result is lost.
                sqlx::query("UPDATE server_module_status SET applied_rev=$3,healthy=true,updated_at=$4 WHERE server_id=$1 AND module=$2")
                    .bind(server_id).bind(&module).bind(rev).bind(now_timestamp()).execute(&mut *tx).await?;
            }
        }
    }
    tx.commit().await?;
    for module in checks {
        // Failure to support a previously advertised capability stays conservative.
        if let Err(error) =
            crate::runtime_control::request_checkpoint(state, server_id, &module).await
        {
            tracing::debug!(%error, "runtime checkpoint request not scheduled");
        }
    }
    if differs {
        let rev: i64 = sqlx::query_scalar("SELECT manifest_rev FROM servers WHERE id=$1")
            .bind(server_id)
            .fetch_one(&state.pool)
            .await?;
        agent_api::notify(
            state,
            server_id,
            Envelope::new("manifest.changed", ManifestChanged { rev: rev as u64 })?,
        )
        .await;
    }
    Ok(())
}

pub async fn record_apply_result(
    state: &AppState,
    server_id: i64,
    result: ApplyResult,
) -> anyhow::Result<()> {
    let rev = i64::try_from(result.rev)?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM deployments WHERE server_id=$1 AND module=$2 AND rev=$3)",
    )
    .bind(server_id)
    .bind(&result.module)
    .bind(rev)
    .fetch_one(&state.pool)
    .await?;
    anyhow::ensure!(exists, "unpublished revision");
    let applied = result.status == ApplyStatus::Applied;
    anyhow::ensure!(!applied || result.healthy, "applied result must be healthy");
    let mut tx = state.pool.begin().await?;
    crate::runtime_control::lock_server(&mut tx, server_id).await?;
    let exact: bool = sqlx::query_scalar("SELECT COALESCE((SELECT checkpoint_required FROM runtime_control_devices WHERE server_id=$1),false)")
        .bind(server_id).fetch_one(&mut *tx).await?;
    if exact {
        tx.commit().await?;
        return crate::runtime_control::record_apply(state, server_id, &result).await;
    }
    sqlx::query("UPDATE server_module_status SET applied_rev=CASE WHEN $4 THEN GREATEST(applied_rev,$3) ELSE applied_rev END,last_result_rev=$3,healthy=$5,last_error=$6,updated_at=$7 WHERE server_id=$1 AND module=$2 AND last_result_rev<=$3 AND applied_rev<=$3")
        .bind(server_id).bind(result.module).bind(rev).bind(applied).bind(result.healthy).bind(result.error.map(|error| error.chars().take(2048).collect::<String>())).bind(now_timestamp()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

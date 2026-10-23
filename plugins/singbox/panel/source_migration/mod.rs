//! Moves numbered subscription sources into ordered sources (ADR 0079 phase 3,
//! step S1b). Nothing here runs on its own: the operator runs
//! `apsp-panel source-migration precheck|apply|rollback` during a maintenance
//! window, and `snapshot`/`compare` record what the migration must not change.

mod import;
mod rehearsal;
mod rollback;

use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
use axum::{Json, extract::State, http::HeaderMap};
use serde::Serialize;
use sqlx::{PgConnection, PgPool};

pub use rehearsal::{compare, snapshot};
pub use rollback::Outcome;

/// Whether ordered sources are authoritative for external nodes.
pub async fn migrated(connection: &mut PgConnection) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT singbox_sources_migrated()")
        .fetch_one(connection)
        .await?)
}

/// What the source pages show: numbered sources stay writable until the
/// migration, and become a read-only archive after it.
#[derive(Serialize)]
pub struct MigrationState {
    pub migrated: bool,
    pub migrated_at: Option<i64>,
}

pub async fn state(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<MigrationState>> {
    auth::require_admin(&state, &headers).await?;
    let migrated_at: Option<i64> =
        sqlx::query_scalar("SELECT migrated_at FROM singbox_source_migration")
            .fetch_one(&state.pool)
            .await?;
    Ok(Json(MigrationState {
        migrated: migrated_at.is_some(),
        migrated_at,
    }))
}

/// Numbered sources stay readable after the migration but accept no writes.
pub(crate) async fn ensure_numbered_writable(pool: &PgPool) -> ApiResult<()> {
    if migrated(&mut *pool.acquire().await?).await? {
        return Err(ApiError::Conflict(
            "数字编号来源已迁移到有序来源，此处只读；请在有序来源中修改".into(),
        ));
    }
    Ok(())
}

/// Sets catalog adoption by public id on the authoritative source table. An
/// imported node also keeps its numbered row, which stays frozen once
/// migrated; a rollback copies adoption back to it.
pub(crate) async fn set_adopted(
    connection: &mut PgConnection,
    public_id: i64,
    adopted: bool,
) -> ApiResult<()> {
    sqlx::query("UPDATE singbox_external_nodes SET adopted=$2 WHERE id=$1 AND NOT singbox_sources_migrated()")
        .bind(public_id)
        .bind(adopted)
        .execute(&mut *connection)
        .await?;
    sqlx::query("UPDATE singbox_ordered_external_nodes SET adopted=$2 WHERE public_id=$1")
        .bind(public_id)
        .bind(adopted)
        .execute(connection)
        .await?;
    Ok(())
}

/// Counts only: a report never contains URLs, credentials or configurations.
#[derive(Clone, Default, Serialize)]
pub struct Report {
    pub migrated: bool,
    pub numbered_sources: i64,
    pub numbered_sources_deleted: i64,
    pub numbered_nodes: i64,
    pub revisions_to_import: i64,
    pub versions_to_import: i64,
    pub external_accesses: i64,
    pub active_numbered_jobs: i64,
    pub pending_numbered_previews: i64,
    pub live_sources_after_merge: i64,
    /// Nodes whose ordered identity key would be shared; they keep their
    /// numbered key, so a later refresh matches them only by provider id.
    pub identity_collisions: i64,
    /// Imported versions the ordered parser cannot type; grants keep using
    /// the original outbound, ordered chains cannot select them.
    pub versions_not_normalized: i64,
    pub referenced_versions_not_normalized: i64,
    /// Grants on nodes whose identity is ambiguous now; ordered sources do not
    /// key ambiguous nodes, so these grants cannot recover by themselves.
    pub ambiguous_nodes_with_access: i64,
    /// Present nodes of inline sources whose imported key the ordered parser
    /// does not produce from the stored content.
    pub inline_identity_changes: i64,
    pub inline_sources_not_reparsed: i64,
    /// Mixed chains keep reading numbered versions until they are converted.
    pub mixed_chains_with_subscription_hops: i64,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
}

impl Report {
    pub fn ready(&self) -> bool {
        self.blockers.is_empty()
    }
}

/// Serializes against every writer of either source table.
async fn lock(connection: &mut PgConnection) -> ApiResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(73201001)")
        .execute(&mut *connection)
        .await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(super::sources::SCHEDULER_LOCK)
        .execute(&mut *connection)
        .await?;
    for key in [73402902, 73402903] {
        sqlx::query("SELECT pg_advisory_xact_lock($1,1)")
            .bind(key)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

/// Reports what `apply` would do, without changing anything.
pub async fn precheck(pool: &PgPool) -> ApiResult<Report> {
    let mut tx = pool.begin().await?;
    lock(&mut tx).await?;
    let report = import::plan(&mut tx).await?.report;
    tx.rollback().await?;
    Ok(report)
}

/// Imports every numbered source in one transaction. Refuses when the
/// precheck finds a blocker or the migration already ran.
pub async fn apply(pool: &PgPool) -> ApiResult<Report> {
    let mut tx = pool.begin().await?;
    lock(&mut tx).await?;
    let plan = import::plan(&mut tx).await?;
    if !plan.report.ready() {
        tx.rollback().await?;
        return Ok(plan.report);
    }
    let report = import::apply(&mut tx, plan).await?;
    tx.commit().await?;
    Ok(report)
}

/// Removes the imported rows and makes numbered sources authoritative again.
/// Refuses while anything created after the migration depends on them.
pub async fn rollback(pool: &PgPool) -> ApiResult<Outcome> {
    let mut tx = pool.begin().await?;
    lock(&mut tx).await?;
    let outcome = rollback::run(&mut tx).await?;
    if outcome.blockers.is_empty() {
        tx.commit().await?;
    } else {
        tx.rollback().await?;
    }
    Ok(outcome)
}

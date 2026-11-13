#![forbid(unsafe_code)]
//! Changes and jobs (ADR 0128 section 3.2): remote commands, terminals, fixed device
//! operations and their reconciliation, runtime operations, the Agent's own upgrade,
//! panel backups, and the jobs, schedules, maintenance windows and bounded remediation
//! that run them. Operations carries out what the owning domain allows and records the
//! attempts; whether an operation is valid stays with that domain.
//!
//! Operations comes last in the dependency order, so it calls the other domains directly.
//! The platform's device work interface (`apsp_panel_host::device_work`) lets earlier
//! domains submit device work here; the application connects it.

pub mod agent_updates;
pub mod backups;
pub mod cloud;
pub mod commands;
pub mod fleet;
pub mod jobs;
pub mod locks;
pub mod retention;
pub mod runtime_operations;

pub(crate) use apsp_governance::{auth, control_center, releases};
pub(crate) use apsp_observability::{incidents, server_monitoring};
pub(crate) use apsp_panel_host::{AppState, digest, error, evidence, text};
#[cfg(test)]
pub(crate) use apsp_panel_host::{config, settings};
pub(crate) use apsp_resources::{
    cloud_resources, device_channel, server_assets, server_lifecycle, servers,
};
use axum::{
    Router,
    routing::{get, post},
};

/// Work operations has not finished, as the system health view counts it.
pub struct Backlog {
    /// Remote commands queued, claimed, running or being cancelled.
    pub commands: i64,
    /// Fixed device operations queued, dispatched or of unknown result, not yet
    /// reconciled.
    pub fleet: i64,
    /// Jobs queued, running, paused, being cancelled or of uncertain result.
    pub jobs: i64,
}

/// Operations' backlog for the system health view.
pub async fn backlog(pool: &sqlx::PgPool) -> error::ApiResult<Backlog> {
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM remote_commands WHERE state IN ('queued','claimed','running','cancel_requested')").fetch_one(pool).await?;
    let fleet: i64 = sqlx::query_scalar("SELECT count(*) FROM fleet_operations WHERE status IN ('queued','dispatched','unknown') AND reconciled_at IS NULL").fetch_one(pool).await?;
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM operations_jobs WHERE status IN ('queued','running','paused','cancel_requested','uncertain')").fetch_one(pool).await?;
    Ok(Backlog {
        commands,
        fleet,
        jobs,
    })
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(fleet::router())
        .merge(jobs::router())
        .merge(cloud::bandwidth::routes())
        .merge(cloud::power::routes())
        .merge(cloud::eip_rotation::routes())
        .route(
            "/api/operations/backups",
            get(backups::recovery::backups).post(backups::recovery::import_backup),
        )
        .route(
            "/api/operations/backups/{id}/drills",
            post(backups::recovery::import_drill),
        )
        .route(
            "/api/operations/backups/{id}/retire",
            post(backups::recovery::retire),
        )
        .route(
            "/api/operations/backup-schedules",
            get(backups::list).post(backups::create),
        )
        .route(
            "/api/operations/backup-schedules/{id}/pause",
            post(backups::pause),
        )
        .route(
            "/api/servers/{id}/commands",
            get(commands::list).post(commands::create),
        )
        .route(
            "/api/servers/{id}/commands/{command}/cancel",
            post(commands::cancel),
        )
        .route(
            "/api/agent/v1/runtime-operations",
            get(runtime_operations::pending),
        )
        .route(
            "/api/agent/v1/runtime-operations/{id}",
            post(runtime_operations::complete),
        )
        .route("/api/agent/v1/commands", get(commands::pending))
        .route("/api/agent/v1/update", get(agent_updates::available))
        .route(
            "/api/agent/v1/commands/lifecycle",
            get(commands::pending_lifecycle),
        )
        .route("/api/agent/v1/commands/{id}/claim", post(commands::claim))
        .route(
            "/api/agent/v1/commands/{id}/control",
            get(commands::control),
        )
        .route(
            "/api/agent/v1/commands/{id}/started",
            post(commands::started),
        )
        .route(
            "/api/agent/v1/commands/{id}",
            post(commands::complete).layer(axum::extract::DefaultBodyLimit::max(4 * 1024 * 1024)),
        )
}

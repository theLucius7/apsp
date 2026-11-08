#![forbid(unsafe_code)]
//! Changes and jobs (ADR 0128 section 3.2): remote commands, terminals, fixed device
//! operations and their reconciliation, runtime operations, and the jobs, schedules,
//! maintenance windows and bounded remediation that run them. Operations carries out what
//! the owning domain allows and records the attempts; whether an operation is valid stays
//! with that domain.
//!
//! Operations comes last in the dependency order, so it calls the other domains directly.
//! The platform's device work interface (`apsp_panel_host::device_work`) lets earlier
//! domains submit device work here; the application connects it.

pub mod commands;
pub mod fleet;
pub mod jobs;
pub mod runtime_operations;

#[cfg(test)]
pub(crate) use apsp_panel_host::config;
pub(crate) use apsp_panel_host::{
    AppState, auth, backups, cloud_resources, control_center, digest, error, incidents,
    server_monitoring, text,
};
use axum::{
    Router,
    routing::{get, post},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(fleet::router())
        .merge(jobs::router())
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

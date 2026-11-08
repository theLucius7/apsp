//! Changes and jobs: remote commands, runtime operations, device operations,
//! terminals and the operations jobs, schedules and maintenance windows.

use axum::{
    Router,
    routing::{get, post},
};

use crate::{AppState, commands, fleet, operations, runtime_operations};

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(fleet::router())
        .merge(operations::router())
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

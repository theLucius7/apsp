//! Resources and devices: servers, enrollment and batch enrollment, asset profiles,
//! lifecycle and migration, cloud resources and retirement.

use axum::{
    Router,
    routing::{get, post},
};

use crate::{AppState, batch_enrollment, cloud_resources, retirement, server_lifecycle, servers};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/servers", get(servers::list).post(servers::create))
        .route(
            "/api/servers/{id}",
            get(servers::get)
                .patch(servers::update)
                .delete(servers::remove),
        )
        .route(
            "/api/servers/{id}/enrollment",
            post(servers::issue_enrollment),
        )
        .route("/api/agent/v1/enroll", post(servers::enroll))
        .route(
            "/api/fleet/templates",
            get(batch_enrollment::templates).post(batch_enrollment::save_template),
        )
        .route("/api/fleet/batch-enrollment", post(batch_enrollment::batch))
        .route(
            "/api/servers/{id}/fleet",
            get(server_lifecycle::get).put(server_lifecycle::save),
        )
        .route(
            "/api/servers/{id}/fleet/lifecycle",
            post(server_lifecycle::lifecycle),
        )
        .route(
            "/api/servers/{id}/fleet/migration-preview",
            post(server_lifecycle::migration_preview),
        )
        .route(
            "/api/servers/{id}/fleet/migrate",
            post(server_lifecycle::migrate),
        )
        .route(
            "/api/servers/{id}/fleet/costs",
            get(server_lifecycle::costs).post(server_lifecycle::add_cost),
        )
        .merge(cloud_resources::hetzner::router())
        .route("/api/operations/cloud", get(cloud_resources::list))
        .route(
            "/api/operations/cloud/{id}/link",
            post(cloud_resources::link),
        )
        .route(
            "/api/operations/cloud/{id}/history",
            get(cloud_resources::history),
        )
        .route(
            "/api/agent/v1/retirement/receipt",
            post(retirement::receipt),
        )
}

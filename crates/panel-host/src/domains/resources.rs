//! Resources and devices: servers, enrollment, assets and retirement.

use axum::{
    Router,
    routing::{get, post},
};

use crate::{AppState, retirement, servers};

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
            "/api/agent/v1/retirement/receipt",
            post(retirement::receipt),
        )
}

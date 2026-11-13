pub(crate) mod access;
mod administration;
mod ceremonies;
mod lines;

pub use lines::{HostNetwork, install as install_host_network};

use crate::AppState;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/access/users/{id}/portal", get(administration::status))
        .route(
            "/api/access/users/{id}/portal/invitation",
            post(administration::invitation),
        )
        .route("/api/access/portal/{account}", get(access::view))
        .route("/api/access/portal/{account}/logout", post(access::logout))
        .route("/api/access/portal/{account}/lines", get(lines::view))
        .route(
            "/api/access/portal/{account}/login/start",
            post(ceremonies::login_start),
        )
        .route(
            "/api/access/portal/{account}/login/finish",
            post(ceremonies::login_finish),
        )
        .route(
            "/api/access/portal/{account}/register/start",
            post(ceremonies::register_start),
        )
        .route(
            "/api/access/portal/{account}/register/finish",
            post(ceremonies::register_finish),
        )
        .route(
            "/api/access/portal/{account}/keys/{id}/remove",
            post(ceremonies::remove),
        )
        .layer(DefaultBodyLimit::max(65536))
}

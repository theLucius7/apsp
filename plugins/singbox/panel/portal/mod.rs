pub(crate) mod access;
mod administration;
mod ceremonies;
mod lines;

use crate::AppState;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/users/{id}/portal", get(administration::status))
        .route(
            "/users/{id}/portal/invitation",
            post(administration::invitation),
        )
        .route("/portal/{account}", get(access::view))
        .route("/portal/{account}/logout", post(access::logout))
        .route("/portal/{account}/lines", get(lines::view))
        .route(
            "/portal/{account}/login/start",
            post(ceremonies::login_start),
        )
        .route(
            "/portal/{account}/login/finish",
            post(ceremonies::login_finish),
        )
        .route(
            "/portal/{account}/register/start",
            post(ceremonies::register_start),
        )
        .route(
            "/portal/{account}/register/finish",
            post(ceremonies::register_finish),
        )
        .route(
            "/portal/{account}/keys/{id}/remove",
            post(ceremonies::remove),
        )
        .layer(DefaultBodyLimit::max(65536))
}

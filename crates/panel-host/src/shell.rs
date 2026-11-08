//! The HTTP shell around the routes the application assembles: the health check,
//! the embedded console as fallback and the request size limit. The application
//! adds its middleware and the state.

use axum::{Router, extract::DefaultBodyLimit, routing::get};

use crate::{AppState, frontend};

pub fn shell(routes: Router<AppState>) -> Router<AppState> {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(routes)
        .fallback(frontend::serve)
        .layer(DefaultBodyLimit::max(1024 * 1024))
}

pub(crate) mod fetch;
mod jobs;
pub mod models;
mod mutations;
mod previews;
mod renewals;
pub use renewals::{RenewalReference, renewal_references};
mod service;
mod snapshots;
mod versions;
pub use versions::{IMPORTED_PARSER_VERSION, Version, versions};
pub mod worker;

pub use jobs::{cancel, get_job, refresh};
pub use mutations::{create, remove, update};
pub use previews::{
    adopt, commit as commit_preview, create as create_preview, remove as remove_preview,
};
pub use service::{get, historical_nodes, history, list, nodes};

pub fn routes() -> axum::Router<crate::AppState> {
    use axum::routing::{get as route_get, post};
    axum::Router::new()
        .route(
            "/api/traffic/ordered-subscription-sources",
            route_get(list).post(create),
        )
        .route(
            "/api/traffic/ordered-subscription-sources/{id}",
            route_get(get).patch(update).delete(remove),
        )
        .route(
            "/api/traffic/ordered-subscription-sources/{id}/refresh",
            post(refresh),
        )
        .route(
            "/api/traffic/ordered-subscription-sources/{id}/nodes",
            route_get(nodes),
        )
        .route(
            "/api/traffic/ordered-subscription-sources/{id}/nodes/{node_id}",
            axum::routing::patch(adopt),
        )
        .route(
            "/api/traffic/ordered-subscription-source-previews",
            post(create_preview),
        )
        .route(
            "/api/traffic/ordered-subscription-source-previews/{id}",
            axum::routing::delete(remove_preview),
        )
        .route(
            "/api/traffic/ordered-subscription-source-previews/{id}/commit",
            post(commit_preview),
        )
        .route(
            "/api/traffic/ordered-subscription-sources/{id}/revisions",
            route_get(history),
        )
        .route(
            "/api/traffic/ordered-subscription-sources/{id}/revisions/{revision}/nodes",
            route_get(historical_nodes),
        )
        .route(
            "/api/traffic/ordered-subscription-source-jobs/{id}",
            route_get(get_job),
        )
        .route(
            "/api/traffic/ordered-subscription-source-jobs/{id}/cancel",
            post(cancel),
        )
        .layer(axum::extract::DefaultBodyLimit::max(3 * 1024 * 1024))
}

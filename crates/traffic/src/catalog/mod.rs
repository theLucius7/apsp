mod clone;
mod mutation;
mod projection;

pub use mutation::{batch_remove, batch_update};
pub(crate) use mutation::{ensure_external_removable, ensure_externals_removable};
pub use projection::list;

pub fn router() -> axum::Router<crate::AppState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/traffic/node-catalog", get(list))
        .route(
            "/api/traffic/node-catalog/batch",
            post(batch_update).patch(batch_update).delete(batch_remove),
        )
        .route("/api/traffic/nodes/{id}/clone", post(clone::clone_node))
}

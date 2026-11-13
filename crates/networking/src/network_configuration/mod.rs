//! Network configuration documents: the store with its revisions, the deployment of a
//! document to its servers and the rewrite of documents when a server is replaced.
//! Certificate and domain documents are kept in this store too; the names domain reads
//! them here and owns their issuance, renewal and deployment.

pub mod documents;
pub mod models;
mod operations;
mod server_migrations;

use crate::AppState;
use axum::Router;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(documents::routes())
        .merge(operations::routes())
        .merge(server_migrations::routes())
}

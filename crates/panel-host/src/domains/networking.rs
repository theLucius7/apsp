//! Basic networking: the network configuration documents and their deployment.

use axum::Router;

use crate::{AppState, network_configuration};

pub fn routes() -> Router<AppState> {
    Router::new().merge(network_configuration::router())
}

//! China reachability monitor.
//!
//! Panel health is observed from outside mainland China, so a node blocked at the
//! border still looks healthy. This module compares TCP connections from mainland
//! and overseas probes through Globalping and classifies each public endpoint. It
//! is opt-in because it sends node addresses and ports to that third party.

mod alerts;
mod api;
mod budget;
mod classify;
mod globalping;
mod measure;
mod store;
mod targets;
#[cfg(test)]
mod tests;
mod worker;

pub use classify::ReachabilityState;

use crate::{AppState, error::ApiResult};
use axum::Router;
use sqlx::PgPool;

pub fn router() -> Router<AppState> {
    api::router()
}

pub async fn run(state: AppState) {
    worker::run(state).await;
}

/// A node as the panel identifies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeReference {
    /// `nodes.id`
    Native(i64),
    /// `singbox_nexus_nodes.id`
    Nexus(i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeReachability {
    pub state: ReachabilityState,
    /// Whether the state is one of the debounced blocked states.
    pub blocked: bool,
    /// A failing observation still waiting for its confirming run.
    pub pending: Option<ReachabilityState>,
    pub checked_at: Option<i64>,
}

/// The latest state of a node's public endpoint, or `None` if the node is unknown
/// or its endpoint was never measured. Callers that gate on this should treat an
/// old `checked_at` as unknown: the monitor may be disabled or deferring.
pub async fn latest_node_state(
    pool: &PgPool,
    node: NodeReference,
) -> ApiResult<Option<NodeReachability>> {
    let endpoint = match node {
        NodeReference::Native(id) => targets::native_endpoint(pool, id).await?,
        NodeReference::Nexus(id) => targets::imported_endpoint(pool, id).await?,
    };
    let Some((host, port)) = endpoint else {
        return Ok(None);
    };
    let Some(row) = store::target_row(pool, &host, port).await? else {
        return Ok(None);
    };
    if row.checked_at.is_none() {
        return Ok(None);
    }
    let state = row.current_state();
    Ok(Some(NodeReachability {
        state,
        blocked: state.is_blocked(),
        pending: row.pending(),
        checked_at: row.checked_at,
    }))
}

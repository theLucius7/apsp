#![forbid(unsafe_code)]
//! The panel application (ADR 0128): it assembles the platform and the domains, connects
//! their typed interfaces and runs their background work. Platform modules and domains are
//! used through their own crates (`apsp_panel_host`, `apsp_traffic`, ...); this crate only
//! exports what it defines.

pub mod agent_api;
mod cloud_overview;
pub mod domains;
mod integration_policy;
pub mod maintenance;
pub mod publisher;
mod wiring;

use apsp_panel_host::{AppState, control_center};

/// Builds the panel router: every domain's routes inside the platform shell, guarded by
/// governance permissions, behind the per-user subscription host gate.
pub fn router(state: AppState) -> axum::Router {
    use apsp_panel_host::domains;
    crate::domains::install();
    let routes = axum::Router::new()
        .merge(apsp_panel_host::agent_api::routes())
        .merge(domains::governance::routes())
        .merge(domains::resources::routes())
        .merge(domains::networking::routes())
        .merge(apsp_observability::routes())
        .merge(apsp_operations::routes())
        .merge(crate::domains::router());
    let guard = axum::middleware::from_fn_with_state(state.clone(), control_center::guard::guard);
    let gate =
        axum::middleware::from_fn_with_state(state.clone(), apsp_access::subscription_hosts::gate);
    apsp_panel_host::shell(routes)
        .layer(guard)
        .with_state(state)
        .layer(gate)
}

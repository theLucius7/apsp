#![forbid(unsafe_code)]
//! The panel application (ADR 0128): it assembles the platform and the domains, connects
//! their typed interfaces and runs their background work.
//!
//! Platform modules are re-exported under their previous public paths; the proxy
//! domains are used through their own crates (`apsp_traffic`, `apsp_access`).

pub use apsp_panel_host::{
    AgentConnection, AppState, DomainStates, agent_updates, artifacts, auth, config,
    control_center, error, frontend, installation, network_configuration, passkeys, releases,
    retirement, server_assets, servers, settings,
};
// Domains that moved out of the platform crate keep their previous public paths.
pub use apsp_costs::exchange;
pub use apsp_observability::{
    dashboard, diagnostic_plugins, diagnostics, incidents, ip_quality, latency_tasks,
    network_workbench, probes, server_monitoring, server_traffic, statistics, telemetry,
    traffic_correction,
};
pub use apsp_operations::jobs as operations;
pub use apsp_operations::{commands, fleet, runtime_operations};

/// Notification delivery from the platform, with alert evaluation, events and rules from
/// observability, under the previous public path.
pub mod notifications {
    pub use apsp_observability::alerts::{evaluate, events, rules};
    pub use apsp_panel_host::notifications::*;
}

pub mod agent_api;
mod cloud_overview;
pub mod domains;
mod integration_policy;
pub mod maintenance;
pub mod publisher;
mod wiring;
pub use apsp_panel_host::schema;

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

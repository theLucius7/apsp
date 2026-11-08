#![forbid(unsafe_code)]
//! The panel application (ADR 0128): it assembles the platform and the domains, connects
//! their typed interfaces and runs their background work.
//!
//! Platform modules and the domain crates are re-exported so the public Rust API
//! (`apsp_panel::auth`, `apsp_panel::plugins::singbox`, ...) stays unchanged.

pub use apsp_panel_host::{
    AgentConnection, AppState, agent_updates, artifacts, auth, config, control_center, dashboard,
    diagnostic_plugins, diagnostics, error, exchange, frontend, installation, ip_quality,
    latency_tasks, network_configuration, network_workbench, notifications, passkeys, probes,
    releases, retirement, runtime_control, runtime_validations, server_assets, server_traffic,
    servers, settings, statistics, telemetry, traffic_correction,
};
// Operations moved to its own crate; its modules keep their previous public paths.
pub use apsp_operations::jobs as operations;
pub use apsp_operations::{commands, fleet, runtime_operations};

pub mod agent_api;
mod integration_policy;
pub mod maintenance;
pub mod plugins;
pub mod publisher;
mod wiring;
pub use apsp_panel_host::schema;

// Compatibility exports preserve the public Rust embedding API.
pub use plugins::singbox::proxy_users as users;
pub use plugins::singbox::{accesses, business, deployments, nodes, subscriptions, usage};

/// Builds the panel router: every domain's routes inside the platform shell, guarded by
/// governance permissions, behind the per-user subscription host gate.
pub fn router(state: AppState) -> axum::Router {
    use apsp_panel_host::domains;
    plugins::install();
    let routes = axum::Router::new()
        .merge(apsp_panel_host::agent_api::routes())
        .merge(domains::governance::routes())
        .merge(domains::resources::routes())
        .merge(domains::networking::routes())
        .merge(domains::traffic::routes())
        .merge(domains::costs::routes())
        .merge(domains::observability::routes())
        .merge(apsp_operations::routes())
        .merge(plugins::router());
    let guard = axum::middleware::from_fn_with_state(state.clone(), control_center::guard::guard);
    let gate = axum::middleware::from_fn_with_state(
        state.clone(),
        plugins::singbox::subscription_hosts::gate,
    );
    apsp_panel_host::shell(routes)
        .layer(guard)
        .with_state(state)
        .layer(gate)
}

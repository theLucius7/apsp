#![forbid(unsafe_code)]
//! Panel assembly: the host crate plus the compiled business plugins.
//!
//! Host modules and the plugin paths are re-exported so the public Rust API
//! (`apsp_panel::auth`, `apsp_panel::plugins::singbox`, ...) stays unchanged.

pub use apsp_panel_host::{
    AgentConnection, AppState, agent_api, agent_updates, artifacts, auth, commands, config,
    control_center, dashboard, diagnostic_plugins, diagnostics, error, exchange, fleet, frontend,
    installation, ip_quality, latency_tasks, maintenance, network_configuration, network_workbench,
    notifications, operations, passkeys, plugin_api, probes, releases, retirement, runtime_control,
    runtime_operations, runtime_validations, server_assets, server_traffic, servers, settings,
    statistics, telemetry, traffic_correction,
};

mod integration_policy;
pub mod plugins;
pub mod publisher;

// Compatibility exports preserve the public Rust embedding API.
pub use plugins::singbox::proxy_users as users;
pub use plugins::singbox::{accesses, business, deployments, nodes, subscriptions, usage};

/// Builds the panel router with every compiled plugin registered.
pub fn router(state: AppState) -> axum::Router {
    apsp_panel_host::router(state, plugins::router())
}

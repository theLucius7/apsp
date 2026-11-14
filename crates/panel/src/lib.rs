#![forbid(unsafe_code)]
//! The panel application (ADR 0128): it assembles the platform and the domains, connects
//! their typed interfaces and runs their background work. Platform modules and domains are
//! used through their own crates (`apsp_panel_host`, `apsp_traffic`, ...); this crate only
//! exports what it defines.

mod cloud_overview;
pub mod domains;
pub mod maintenance;
mod moved_paths;
pub mod publisher;
mod search;
mod server_export;
mod system_health;
mod wiring;

use apsp_governance::control_center;
use apsp_panel_host::{AppState, config::Config};

/// The shared state of a running panel: the typed interfaces are connected, governance is
/// prepared and the registry of connected devices exists, so every later copy of the
/// state (a portal request scoped to a personal address, for one) shares it.
pub async fn state(pool: sqlx::PgPool, config: Config) -> anyhow::Result<AppState> {
    domains::install();
    let state = apsp_governance::state(pool, config).await?;
    apsp_resources::device_channel::devices(&state);
    Ok(state)
}

/// Builds the panel router: every domain's routes inside the platform shell, guarded by
/// governance permissions, behind the per-user subscription host gate. Requests to an
/// address the console API had before are answered at its current address.
pub fn router(state: AppState) -> axum::Router {
    let guard = axum::middleware::from_fn_with_state(state.clone(), control_center::guard::guard);
    let gate =
        axum::middleware::from_fn_with_state(state.clone(), apsp_access::subscription_hosts::gate);
    let panel = apsp_panel_host::shell(domains::router())
        .layer(guard)
        .with_state(state)
        .layer(gate);
    // A layer runs after its router has matched the request. The rewrite of former
    // addresses must run before that, so it sits on an outer router whose only route is
    // the panel itself.
    axum::Router::new()
        .fallback_service(panel)
        .layer(axum::middleware::from_fn(moved_paths::current))
}

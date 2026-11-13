#![forbid(unsafe_code)]
//! Basic networking (ADR 0128): how data reaches its target on the managed machines. The
//! network configuration documents with their deployment (`network_configuration`) and
//! the cloud security groups (`security_groups`).

pub mod locks;
pub mod network_configuration;
pub mod security_groups;

pub(crate) use apsp_governance::{auth, control_center};
#[cfg(test)]
pub(crate) use apsp_panel_host::config;
pub(crate) use apsp_panel_host::{AppState, device_work, digest, error};
pub(crate) use apsp_resources::{retirement, server_lifecycle, servers};

/// The domain's routes: network configuration and cloud security groups.
pub fn routes() -> axum::Router<AppState> {
    network_configuration::router().merge(security_groups::routes())
}

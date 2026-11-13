#![forbid(unsafe_code)]
//! Resources and devices (ADR 0128): what the panel manages and the state it is in.
//! Servers with their enrollment and installation, the device channel that connects
//! their Agents, asset profiles, lifecycle and retirement, and the cloud accounts and
//! resources (`cloud`, `cloud_resources`).

pub mod batch_enrollment;
pub mod bootstrap;
pub mod cloud;
pub mod cloud_resources;
pub mod device_artifacts;
pub mod device_channel;
pub mod installation;
pub mod retirement;
mod routes;
pub mod server_assets;
pub mod server_lifecycle;
pub mod server_observation;
pub mod servers;

pub use routes::routes;

pub(crate) use apsp_governance::{auth, control_center, releases};
#[cfg(test)]
pub(crate) use apsp_panel_host::config;
pub(crate) use apsp_panel_host::{AppState, device_work, error, evidence, ports, text, tokens};

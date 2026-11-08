#![forbid(unsafe_code)]
//! Panel platform: shared technical services, the device channel and the HTTP shell
//! (ADR 0128). It names no domain; code of domains that has not moved to its own crate
//! yet still lives here, registered by owner, and each such domain declares its routes
//! in [`domains`]. The `apsp-panel` application assembles the routes and connects the
//! typed interfaces of [`ports`].

pub mod agent_api;
pub mod agent_updates;
pub mod artifacts;
pub mod auth;
pub mod backups;
pub mod batch_enrollment;
pub mod cloud_resources;
pub mod config;
pub mod control_center;
pub mod dashboard;
pub mod device_config;
pub mod device_work;
pub mod diagnostic_plugins;
pub mod diagnostics;
pub mod digest;
pub mod domains;
pub mod error;
pub mod exchange;
pub mod frontend;
pub mod incidents;
pub mod installation;
pub mod ip_quality;
pub mod latency_tasks;
pub mod maintenance;
pub mod net;
pub mod network_configuration;
pub mod network_workbench;
pub mod notifications;
pub mod passkeys;
pub mod ports;
pub mod probes;
pub mod releases;
pub mod retirement;
pub mod runtime_control;
pub mod runtime_validations;
pub mod schema;
pub mod server_assets;
pub mod server_lifecycle;
pub mod server_monitoring;
pub mod server_traffic;
pub mod servers;
pub mod settings;
mod shell;
pub mod statistics;
pub mod telemetry;
pub mod text;
pub mod tokens;
pub mod traffic_correction;

mod state;

pub use shell::shell;
pub use state::{AgentConnection, AppState};

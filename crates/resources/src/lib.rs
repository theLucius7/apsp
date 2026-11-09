#![forbid(unsafe_code)]
//! Resources and devices (ADR 0128): what the panel manages and the state it is in. This
//! crate holds the cloud accounts and resources (`cloud`); the rest of the domain (servers,
//! enrollment, retirement) still lives in the host until it moves here in stage B2.

pub mod cloud;

pub(crate) use apsp_panel_host::{AppState, auth, control_center, error};

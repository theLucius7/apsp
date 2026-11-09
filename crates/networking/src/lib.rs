#![forbid(unsafe_code)]
//! Basic networking (ADR 0128). This crate holds the cloud security groups
//! (`security_groups`); the rest of the domain still lives in the host until it moves here
//! in stage B2.

pub mod security_groups;

pub(crate) use apsp_panel_host::{AppState, control_center, error};
#[cfg(test)]
pub(crate) use apsp_panel_host::{auth, config};

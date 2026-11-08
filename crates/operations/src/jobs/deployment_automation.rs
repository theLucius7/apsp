//! Deployment steps that jobs run through the traffic service.
//!
//! Operations decides when a step runs and records the attempt; whether the
//! deployment is valid and how it is applied stays with the traffic service, which
//! these functions call.

pub use apsp_plugin_singbox::host_support::runtime_step_policy;
pub use apsp_plugin_singbox::operations_workflows::{
    automation_candidate, automation_deployment_receipt, automation_dispatch_matches_tx,
    cancel_automation_deployment_tx, enqueue_automation_deployment_tx,
    reconcile_automation_deployment_tx, request_automation_deployment_checkpoint_tx,
};

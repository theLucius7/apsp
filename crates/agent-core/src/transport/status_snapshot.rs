use super::Runtime;
use crate::state::Locked;
use anyhow::Result;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::atomic::Ordering};

pub(super) fn snapshot(runtime: &Runtime) -> Result<Value> {
    let applied = runtime.applied()?;
    let state = runtime.state.locked()?;
    let mut healthy = BTreeMap::new();
    for module in runtime.modules.iter() {
        healthy.insert(
            module.clone(),
            state
                .get_json::<bool>(&format!("health:{module}"))?
                .unwrap_or(false),
        );
    }
    let telemetry = runtime.telemetry.borrow();
    let oversized_usage_batches = state.oversized_usage_count()?;
    Ok(json!({
        "agent_version": runtime.agent_version, "pid": std::process::id(),
        "connected": runtime.connected.load(Ordering::Relaxed),
        "applied": applied, "healthy": healthy,
        "pending_batches": state.pending_usage_count()?,
        "pending_telemetry": state.pending_telemetry_count()?,
        "metrics_sampled_at": telemetry.sample.as_ref().map(|sample| sample.sampled_at),
        "telemetry_stale": telemetry.timed_out() || telemetry.error.is_some(),
        "telemetry_error": telemetry.error,
        "oversized_usage_batches": oversized_usage_batches,
        "usage_outbox_error": (oversized_usage_batches > 0).then_some(
            "Legacy usage batches exceed the wire byte budget; ledger reconciliation is required before acknowledging them."
        ),
    }))
}

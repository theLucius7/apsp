//! The proxy publication cycle (every second). It runs work of two owners in one order:
//! package eligibility (access) before ordered path transitions and configuration
//! publication (traffic), then operation retention every 30 s and device-limit policies
//! every 10 s (access). One heartbeat, `sing-box`, covers the whole cycle.

use apsp_access as access;
use apsp_panel_host::{AppState, control_center};
use apsp_protocol::now_timestamp;
use apsp_traffic as traffic;
use std::time::Duration;

pub async fn run(state: AppState) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut next_maintenance = 0;
    let mut maintenance_healthy = false;
    let mut next_admission = 0;
    loop {
        interval.tick().await;
        let mut failed = Vec::new();
        if let Err(error) = access::entitlements::refresh(&state.pool, now_timestamp()).await {
            tracing::error!(%error, "package eligibility refresh failed; will retry");
            failed.push("entitlements");
        }
        if let Err(_error) = traffic::ordered_paths::lifecycle::tick(&state).await {
            tracing::error!("ordered path transition failed; durable work retained");
            failed.push("ordered-path-lifecycle");
        }
        if let Err(error) = traffic::publisher::publish_due(&state).await {
            tracing::error!(%error, "configuration publication failed; pending work retained");
            failed.push("configuration-publication");
        }
        if now_timestamp() >= next_maintenance {
            maintenance_healthy = match access::operations_workflows::maintenance(&state).await {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(%error, "proxy operation retention failed; work retained");
                    false
                }
            };
            next_maintenance = now_timestamp() + 30;
        }
        if !maintenance_healthy {
            failed.push("operation-retention");
        }
        if now_timestamp() >= next_admission {
            if let Err(error) = access::nexus_sessions::push_policies(&state).await {
                tracing::warn!(%error, "device limit policies were not sent; will retry");
                failed.push("device-limits");
            }
            next_admission = now_timestamp() + 10;
        }
        if let Err(error) = control_center::system::heartbeat(
            &state.pool,
            "sing-box",
            if failed.is_empty() { "healthy" } else { "failed" },
            serde_json::json!({"source":"plugin-publisher-cycle","failed_steps":failed,"checked_steps":["entitlements","ordered-path-lifecycle","configuration-publication"],"retention_checked_every_seconds":30}),
        )
        .await
        {
            tracing::warn!(%error, "proxy publisher heartbeat unavailable");
        }
    }
}

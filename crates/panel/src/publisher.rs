//! The proxy publication cycle (every second). It runs work of two owners in one order:
//! package eligibility (access) before ordered path transitions and configuration
//! publication (traffic), then every 30 s rollout inspection results (traffic) and record
//! retention (access), and every 10 s device-limit policies (access). One heartbeat,
//! `sing-box`, covers the whole cycle.
//!
//! Every user's entitlement is evaluated once per cycle, by the eligibility refresh; host
//! membership and device-limit policies take its allowed users. When the refresh fails
//! they evaluate entitlements themselves, as they would on their own.

use apsp_access as access;
use apsp_panel_host::{
    AppState,
    maintenance::{Cadence, Failure, Pass, periodic},
};
use apsp_protocol::now_timestamp;
use apsp_traffic as traffic;
use std::time::Duration;

pub async fn run(state: AppState) {
    let cycle = Cycle {
        state: state.clone(),
        next_maintenance: 0,
        maintenance_healthy: false,
        next_admission: 0,
    };
    periodic(
        "sing-box",
        state.pool,
        Cadence::every(Duration::from_secs(1)),
        cycle,
        |cycle| Box::pin(cycle.pass()),
    )
    .await
}

/// When the slower steps run next, and how the last retention step went.
struct Cycle {
    state: AppState,
    next_maintenance: i64,
    maintenance_healthy: bool,
    next_admission: i64,
}

impl Cycle {
    async fn pass(&mut self) -> Pass {
        let state = &self.state;
        let mut failed = Vec::new();
        let allowed = match access::entitlements::refresh(&state.pool, now_timestamp()).await {
            Ok(allowed) => Some(allowed),
            Err(error) => {
                tracing::error!(%error, "package eligibility refresh failed; will retry");
                failed.push("entitlements");
                None
            }
        };
        if let Err(_error) = traffic::ordered_paths::lifecycle::tick(state).await {
            tracing::error!("ordered path transition failed; durable work retained");
            failed.push("ordered-path-lifecycle");
        }
        if let Err(error) = traffic::publisher::publish_due(state, allowed.as_deref()).await {
            tracing::error!(%error, "configuration publication failed; pending work retained");
            failed.push("configuration-publication");
        }
        if now_timestamp() >= self.next_maintenance {
            // Rollout inspection results (traffic) and record retention (access).
            let results = [
                traffic::runtime_workflows::maintenance(state).await,
                access::operations_workflows::maintenance(state).await,
            ];
            self.maintenance_healthy = true;
            for error in results.into_iter().filter_map(Result::err) {
                tracing::warn!(%error, "proxy operation retention failed; work retained");
                self.maintenance_healthy = false;
            }
            self.next_maintenance = now_timestamp() + 30;
        }
        if !self.maintenance_healthy {
            failed.push("operation-retention");
        }
        if now_timestamp() >= self.next_admission {
            if let Err(error) =
                access::nexus::sessions::push_policies(state, allowed.as_deref()).await
            {
                tracing::warn!(%error, "device limit policies were not sent; will retry");
                failed.push("device-limits");
            }
            self.next_admission = now_timestamp() + 10;
        }
        let details = serde_json::json!({"source":"publication-cycle","failed_steps":failed,"checked_steps":["entitlements","ordered-path-lifecycle","configuration-publication"],"retention_checked_every_seconds":30});
        if failed.is_empty() {
            Ok(details)
        } else {
            Err(Failure::logged(details))
        }
    }
}

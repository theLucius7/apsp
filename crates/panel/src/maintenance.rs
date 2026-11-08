//! The application's 30-second maintenance cycle: each domain's expiry and retention
//! step, alert evaluation and notification delivery, recorded under one heartbeat.
//! The platform provides the supervisor.

pub use apsp_panel_host::maintenance::supervise;

use apsp_panel_host::AppState;
use std::time::Duration;

pub async fn run(state: AppState) {
    let mut poll = tokio::time::interval(Duration::from_secs(30));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        poll.tick().await;
        let mut healthy = true;
        if let Err(error) = apsp_observability::diagnostics::expire(&state).await {
            healthy = false;
            tracing::error!(%error, "diagnostic expiry cleanup failed");
        }
        if let Err(error) =
            apsp_panel_host::server_assets::renew_due(&state.pool, apsp_protocol::now_timestamp())
                .await
        {
            healthy = false;
            tracing::error!(%error, "server expiry renewal failed");
        }
        if let Err(error) = apsp_panel_host::auth::purge_expired_sessions(
            &state.pool,
            apsp_protocol::now_timestamp(),
        )
        .await
        {
            healthy = false;
            tracing::error!(%error, "expired session cleanup failed");
        }
        if let Err(error) = apsp_panel_host::servers::purge_expired_enrollments(
            &state.pool,
            apsp_protocol::now_timestamp(),
        )
        .await
        {
            tracing::error!(%error, "expired enrollment token cleanup failed");
        }
        if let Err(error) =
            apsp_operations::commands::purge_finished(&state.pool, apsp_protocol::now_timestamp())
                .await
        {
            tracing::error!(%error, "finished command retention failed");
        }
        let now = apsp_protocol::now_timestamp();
        if let Err(error) =
            apsp_observability::alerts::evaluate(&state.pool, state.started_at, now).await
        {
            healthy = false;
            tracing::error!(%error, "server alert evaluation failed");
        }
        if let Err(error) = apsp_panel_host::notifications::dispatch(&state.pool, now).await {
            healthy = false;
            tracing::error!(%error, "notification delivery failed");
        }
        if let Err(error) = apsp_panel_host::control_center::system::heartbeat(
            &state.pool,
            "maintenance",
            if healthy { "healthy" } else { "failed" },
            serde_json::json!({"source":"expiry-alert-delivery-cycle"}),
        )
        .await
        {
            tracing::warn!(%error,"maintenance heartbeat persistence failed");
        }
    }
}

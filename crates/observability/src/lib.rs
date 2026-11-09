#![forbid(unsafe_code)]
//! Observability evidence and events (ADR 0128 section 3.2): telemetry and history,
//! network counters and corrections, probes and latency tasks, the shared diagnostic job
//! service and its tools (IP quality, NodeQuality, TCP quality, network checks), alert
//! rules and events, incidents, server health and event timelines, statistics and the
//! dashboard. Observability records facts and the reasons for its judgements; the
//! configuration and rules of what it observes stay with their owners.
//!
//! Earlier domains report events and incidents through the platform's `evidence`
//! interface and reach server observation through `server_observation` in resources;
//! the application connects both to this crate.

pub mod alerts;
pub mod cloud_notices;
pub mod dashboard;
pub mod diagnostic_plugins;
pub mod diagnostics;
pub mod incidents;
pub mod ip_quality;
pub mod latency_tasks;
pub mod network_workbench;
pub mod probes;
pub mod reachability;
pub mod server_monitoring;
pub mod server_traffic;
pub mod statistics;
pub mod telemetry;
pub mod traffic_correction;

// Platform modules and earlier domains that the code names as `crate::...`.
#[cfg(test)]
pub(crate) use apsp_panel_host::config;
pub(crate) use apsp_panel_host::{
    AppState, agent_api, artifacts, auth, control_center, device_work, error, net, notifications,
    server_assets, servers, settings, text,
};
use axum::{
    Router,
    routing::{get, post},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/statistics", get(statistics::summary))
        .route(
            "/api/plugins/sing-box/statistics",
            get(statistics::proxy_usage::summary),
        )
        .route("/api/notifications", get(alerts::events::list))
        .route(
            "/api/alert-rules",
            get(alerts::rules::list).post(alerts::rules::create),
        )
        .route(
            "/api/alert-rules/{id}",
            axum::routing::patch(alerts::rules::update).delete(alerts::rules::remove),
        )
        .route("/api/operations/incidents", get(incidents::list))
        .route("/api/operations/incidents/{id}", get(incidents::detail))
        .route(
            "/api/operations/incidents/{id}/action",
            post(incidents::action),
        )
        .route(
            "/api/operations/incidents/{id}/assignees",
            get(incidents::assignees),
        )
        .route("/api/fleet/health", get(server_monitoring::health))
        .route(
            "/api/servers/{id}/fleet/events",
            get(server_monitoring::events),
        )
        .route(
            "/api/servers/{id}/fleet/compare",
            get(server_monitoring::compare),
        )
        .merge(network_workbench::router())
        .merge(dashboard::routes())
        .route(
            "/api/servers/{id}/traffic-correction",
            post(traffic_correction::correct),
        )
        .route(
            "/api/servers/{id}/agent-settings",
            get(telemetry::settings).patch(telemetry::update_settings),
        )
        .route("/api/servers/{id}/metrics", get(telemetry::history))
        .route(
            "/api/servers/{id}/history",
            get(telemetry::aggregate_history),
        )
        .route(
            "/api/servers/{id}/telemetry-settings",
            get(telemetry::storage_settings).patch(telemetry::update_storage_settings),
        )
        .route(
            "/api/telemetry/policy",
            get(telemetry::policy).patch(telemetry::update_policy),
        )
        .route(
            "/api/servers/{id}/probes",
            get(probes::list).post(probes::create),
        )
        .route(
            "/api/servers/{id}/probes/{probe}",
            axum::routing::patch(probes::update).delete(probes::remove),
        )
        .route("/api/servers/{id}/probe-results", get(probes::history))
        .route("/api/probes/overview", get(probes::overview))
        .route(
            "/api/latency-tasks",
            get(latency_tasks::list).post(latency_tasks::create),
        )
        .route(
            "/api/latency-tasks/{id}",
            axum::routing::patch(latency_tasks::update).delete(latency_tasks::remove),
        )
        .route(
            "/api/servers/{id}/diagnostics",
            get(diagnostics::service::get),
        )
        .route(
            "/api/servers/{id}/diagnostics/{plugin}",
            post(diagnostics::service::create),
        )
        .route("/api/servers/{id}/ip-quality", get(ip_quality::get))
        .route(
            "/api/servers/{id}/ip-quality/node-query",
            post(diagnostic_plugins::nodequality::node_queries::create),
        )
        .route(
            "/api/servers/{id}/ip-quality/refresh",
            post(ip_quality::refresh),
        )
        .route(
            "/api/servers/{id}/node-quality",
            get(diagnostics::legacy_get),
        )
        .route(
            "/api/servers/{id}/node-quality/refresh",
            post(ip_quality::refresh),
        )
        .route(
            "/api/servers/{id}/node-quality/reports",
            get(diagnostics::get).post(diagnostics::create),
        )
        .route(
            "/api/servers/{id}/diagnostics/{job}/cancel",
            post(diagnostics::cancellation::request),
        )
        .route(
            "/api/plugins/tcpquality/servers/{id}/targets",
            get(diagnostic_plugins::tcpquality::list_targets),
        )
        .route(
            "/api/plugins/tcpquality/servers/{id}/targets/{probe}",
            axum::routing::patch(diagnostic_plugins::tcpquality::set_region),
        )
        .route("/api/agent/v1/settings", get(telemetry::agent_settings))
        .route(
            "/api/agent/v1/telemetry-settings",
            get(telemetry::agent_storage_settings),
        )
        .route("/api/agent/v1/telemetry", post(telemetry::ingest))
        .route("/api/agent/v1/telemetry/live", post(telemetry::live))
        .route("/api/agent/v1/probes", get(probes::agent_list))
        .route("/api/agent/v1/probe-lease", get(probes::agent_lease))
        .route(
            "/api/agent/v1/probes/authorized",
            get(probes::agent_authorized_list),
        )
        .route("/api/agent/v1/probe-results", post(probes::ingest))
        .route("/api/agent/v1/diagnostics", get(diagnostics::pending))
        .route(
            "/api/agent/v1/diagnostics/cancellations",
            get(diagnostics::cancellation::pending),
        )
        .route(
            "/api/agent/v1/diagnostics/{id}/cancel-confirmation",
            post(diagnostics::cancellation::confirm),
        )
        .route("/api/agent/v1/diagnostics/{id}", post(diagnostics::update))
        .route(
            "/api/agent/v1/diagnostics/{id}/sections",
            post(diagnostics::upload_section),
        )
}

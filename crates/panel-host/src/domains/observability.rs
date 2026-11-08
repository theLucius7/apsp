//! Observability: telemetry, network counters and corrections, probes and latency
//! tasks, diagnostics, network checks, alerts and incidents, server health and event
//! timelines, statistics and the dashboard.

use axum::{
    Router,
    routing::{get, post},
};

use crate::{
    AppState, dashboard, diagnostic_plugins, diagnostics, incidents, ip_quality, latency_tasks,
    network_workbench, notifications, probes, server_monitoring, statistics, telemetry,
    traffic_correction,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/statistics", get(statistics::summary))
        .route("/api/notifications", get(notifications::events::list))
        .route(
            "/api/alert-rules",
            get(notifications::rules::list).post(notifications::rules::create),
        )
        .route(
            "/api/alert-rules/{id}",
            axum::routing::patch(notifications::rules::update).delete(notifications::rules::remove),
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

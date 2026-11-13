//! The system health view (系统自检): the panel itself as governance observes it, the work
//! operations and observability have not finished, and the panel backups. It joins
//! several owners, so it belongs to the application (ADR 0128).

use apsp_governance::control_center::{require_owner, system};
use apsp_observability as observability;
use apsp_operations as operations;
use apsp_panel_host::{AppState, error::ApiResult};
use axum::{Json, extract::State, http::HeaderMap};
use serde_json::{Value, json};

pub async fn health(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    require_owner(&state, &headers).await?;
    let panel = system::panel_health(&state, crate::domains::HEARTBEATS).await?;
    let work = operations::backlog(&state.pool).await?;
    let evidence = observability::backlog(&state.pool).await?;
    let backups = operations::backups::summary(&state.pool).await?;
    Ok(Json(json!({
        "version": panel.version,
        "sampled_at": panel.sampled_at,
        "uptime_seconds": panel.uptime_seconds,
        "database": panel.database,
        "disk": panel.disk,
        "backlog": {
            "commands": work.commands,
            "diagnostics": evidence.diagnostics,
            "notifications": evidence.notifications,
            "fleet": work.fleet,
            "operations": work.jobs,
        },
        "workers": panel.workers,
        "independent_observers": panel.independent_observers,
        "backup": {
            "status": if backups.latest.is_some() { "recorded" } else { "unknown" },
            "latest": backups.latest,
            "schedules": backups.schedules,
            "source": "local-backup-ledger",
        },
        "update": panel.update,
    })))
}

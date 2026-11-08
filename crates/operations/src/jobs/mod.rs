//! Jobs: templates, previews and confirmed jobs across servers, schedules, maintenance
//! windows, bounded remediation and the panel backup step, with their execution
//! history and reconciliation of unknown results.

mod api;
pub mod deployment_automation;
mod model;
mod panel_backup;
mod planning;
mod remediation;
mod runtime_reconciliation;
mod typed_steps;
mod worker;

pub use model::{Plan, Step};
pub use worker::{run, tick};

use crate::AppState;
use axum::{
    Router,
    routing::{get, post},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/operations/templates",
            get(api::templates).post(api::save_template),
        )
        .route("/api/operations/jobs", get(api::jobs))
        .route("/api/operations/preview", post(api::preview))
        .route("/api/operations/jobs/{id}", get(api::detail))
        .route("/api/operations/jobs/{id}/confirm", post(api::confirm))
        .route("/api/operations/jobs/{id}/cancel", post(api::cancel))
        .route("/api/operations/jobs/{id}/resume", post(api::resume))
        .route("/api/operations/jobs/{id}/reconcile", post(api::reconcile))
        .route(
            "/api/operations/jobs/{id}/inspection",
            post(api::inspect_reconciliation),
        )
        .route(
            "/api/operations/jobs/{id}/panel-backup/reconcile",
            post(panel_backup::reconcile),
        )
        .route(
            "/api/operations/jobs/{id}/runtime-reconcile/checkpoint",
            post(runtime_reconciliation::checkpoint),
        )
        .route(
            "/api/operations/jobs/{id}/runtime-reconcile",
            post(runtime_reconciliation::reconcile),
        )
        .route(
            "/api/operations/schedules",
            get(api::schedules).post(api::save_schedule),
        )
        .route(
            "/api/operations/schedules/{id}/pause",
            post(api::pause_schedule),
        )
        .route(
            "/api/operations/remediation",
            get(remediation::list).post(remediation::create),
        )
        .route(
            "/api/operations/remediation/{id}/pause",
            post(remediation::pause),
        )
        .route(
            "/api/operations/maintenance",
            get(api::maintenance).post(api::save_maintenance),
        )
}

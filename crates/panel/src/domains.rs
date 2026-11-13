//! Every domain's routes and background work, in dependency order (ADR 0128). The typed
//! interfaces between owners are connected in `wiring`.

use apsp_access as access;
use apsp_costs as costs;
use apsp_governance as governance;
use apsp_naming as naming;
use apsp_networking as networking;
use apsp_observability as observability;
use apsp_operations as operations;
use apsp_panel_host::{AppState, maintenance::supervise};
use apsp_resources as resources;
use apsp_traffic as traffic;
use axum::{Router, routing::get};
use sqlx::PgPool;

/// The heartbeat names the workers below write, as the system health view lists them.
const HEARTBEATS: &[&str] = &[
    "maintenance",
    "telemetry-history",
    "control-center",
    "network-workbench",
    "operations",
    "operations-backups",
    "operations-job-backups",
    "network-certificates",
    "ddns",
    "expenses",
    "alicloud",
    "alicloud-cost-cache",
    "alicloud-power",
    "alicloud-eip-rotation",
    "sing-box",
];

/// Connects the typed interfaces of the compiled domains and declares the workers the
/// health view expects; repeated calls are harmless.
pub fn install() {
    crate::wiring::install();
    governance::control_center::system::expect_workers(HEARTBEATS);
}

/// Every domain's routes, with the two views the application composes across domains.
pub fn router() -> Router<AppState> {
    install();
    Router::new()
        .merge(governance::routes())
        .merge(resources::routes())
        .merge(networking::routes())
        .merge(naming::routes())
        .merge(traffic::routes())
        .merge(access::routes())
        .merge(costs::routes())
        .merge(observability::routes())
        .merge(operations::routes())
        .merge(crate::cloud_overview::routes())
        .route("/api/control-center/search", get(crate::search::search))
}

/// Runs every background worker until the returned future is dropped. Each worker is
/// supervised on its own, so one panic neither stops the others nor goes unnoticed; the
/// supervisor names are kept so logs and heartbeats stay continuous.
pub async fn run(state: AppState) {
    install();
    let pool = state.pool.clone();
    tokio::join!(
        supervise("maintenance", {
            let state = state.clone();
            move || crate::maintenance::run(state.clone())
        }),
        supervise("control-center", {
            let state = state.clone();
            move || governance::control_center::run(state.clone())
        }),
        supervise("ddns", {
            let pool = pool.clone();
            move || naming::run(pool.clone())
        }),
        supervise("network-certificates", {
            let state = state.clone();
            move || naming::run_certificates(state.clone())
        }),
        supervise("sing-box", {
            let state = state.clone();
            move || proxy(state.clone())
        }),
        supervise("expenses", {
            let state = state.clone();
            move || costs::run(state.clone())
        }),
        supervise("exchange-rates", {
            let pool = pool.clone();
            move || costs::exchange::run(pool.clone())
        }),
        supervise("telemetry-history", {
            let pool = pool.clone();
            move || telemetry_history(pool.clone())
        }),
        supervise("network-workbench", {
            let state = state.clone();
            move || network_workbench(state.clone())
        }),
        supervise("operations", {
            let state = state.clone();
            move || operations::jobs::run(state.clone())
        }),
        supervise("alicloud", move || operations::cloud::worker::run(
            pool.clone()
        )),
    );
}

/// The proxy network's workers: the publication cycle, external sources, member
/// accounts, personal subscription hosts and China reachability checks.
async fn proxy(state: AppState) {
    tokio::join!(
        crate::publisher::run(state.clone()),
        traffic::run(state.clone()),
        access::run(state.clone()),
        observability::reachability::run(state)
    );
}

async fn telemetry_history(pool: PgPool) {
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        timer.tick().await;
        let result = observability::telemetry::maintain(&pool).await;
        let status = if result.is_ok() { "healthy" } else { "failed" };
        if let Err(error) = result {
            tracing::warn!(%error, "telemetry history maintenance failed");
        }
        if let Err(error) = governance::control_center::system::heartbeat(
            &pool,
            "telemetry-history",
            status,
            serde_json::json!({"source":"history-maintenance-cycle"}),
        )
        .await
        {
            tracing::warn!(%error,"telemetry worker heartbeat failed");
        }
    }
}

async fn network_workbench(state: AppState) {
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(5));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        timer.tick().await;
        let result = observability::network_workbench::tick(&state).await;
        let status = if result.is_ok() { "healthy" } else { "failed" };
        if let Err(error) = result {
            tracing::warn!(%error,"network workbench dispatch failed");
        }
        if let Err(error) = governance::control_center::system::heartbeat(
            &state.pool,
            "network-workbench",
            status,
            serde_json::json!({"source":"dispatch-cycle"}),
        )
        .await
        {
            tracing::warn!(%error,"network workbench heartbeat failed");
        }
    }
}

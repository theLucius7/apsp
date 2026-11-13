//! Every domain's routes and background work, in dependency order (ADR 0128). The typed
//! interfaces between owners are connected in `wiring`.

use apsp_access as access;
use apsp_costs as costs;
use apsp_governance as governance;
use apsp_naming as naming;
use apsp_networking as networking;
use apsp_observability as observability;
use apsp_operations as operations;
use apsp_panel_host::{
    AppState,
    maintenance::{Cadence, outcome, periodic, supervise},
};
use apsp_resources as resources;
use apsp_traffic as traffic;
use axum::{Router, routing::get};
use sqlx::PgPool;
use std::time::Duration;

/// The heartbeat names the workers below write, as the system health view lists them.
/// Every background loop runs on the platform's periodic executor under exactly one of
/// these names; the tests below hold this list to the loops.
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
    "exchange-rates",
    "alicloud",
    "alicloud-cost-cache",
    "alicloud-power",
    "alicloud-eip-rotation",
    "sing-box",
    "subscription-sources",
    "member-messages",
    "subscription-hosts",
    "china-reachability",
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
    periodic(
        "telemetry-history",
        pool.clone(),
        Cadence::every(Duration::from_secs(60)),
        pool,
        |pool| {
            Box::pin(async move {
                outcome(
                    observability::telemetry::maintain(pool).await,
                    serde_json::json!({"source":"history-maintenance-cycle"}),
                )
            })
        },
    )
    .await
}

async fn network_workbench(state: AppState) {
    periodic(
        "network-workbench",
        state.pool.clone(),
        Cadence::every(Duration::from_secs(5)),
        state,
        |state| {
            Box::pin(async move {
                outcome(
                    observability::network_workbench::tick(state).await,
                    serde_json::json!({"source":"dispatch-cycle"}),
                )
            })
        },
    )
    .await
}

#[cfg(test)]
mod tests;

//! The domain crates' routes and background work (ADR 0128). The typed interfaces between
//! owners are connected in `wiring`.

use apsp_access as access;
use apsp_costs as costs;
use apsp_naming as naming;
use apsp_networking as networking;
use apsp_observability as observability;
use apsp_operations as operations;
use apsp_panel_host::AppState;
use apsp_resources as resources;
use apsp_traffic as traffic;
use axum::Router;

/// Connects the typed interfaces of the compiled domains; repeated calls are harmless.
pub fn install() {
    crate::wiring::install();
}

/// The proxy console API keeps its `/api/plugins/sing-box` prefix (ADR 0131: it changes
/// once the Nexus bridge exits); subscriptions and the device channel keep their paths.
pub fn router() -> Router<AppState> {
    install();
    Router::new()
        .nest(
            "/api/plugins/sing-box",
            traffic::management()
                .merge(access::management())
                .merge(observability::reachability::router()),
        )
        .merge(access::subscription_routes())
        .merge(traffic::device_routes())
        .merge(naming::routes())
        .merge(costs::routes())
        // Cloud capabilities by domain: accounts and resources, bandwidth changes, power,
        // address rotation, CDT traffic history, security groups and the combined view.
        // They keep the `/api/plugins/alicloud` prefix until the prefixes change (ADR 0131).
        .merge(resources::cloud::api::routes())
        .merge(operations::cloud::bandwidth::routes())
        .merge(operations::cloud::power::routes())
        .merge(operations::cloud::eip_rotation::routes())
        .merge(costs::cloud::traffic_history::routes())
        .merge(networking::security_groups::routes())
        .merge(crate::cloud_overview::routes())
}

pub async fn run(state: AppState) {
    use apsp_panel_host::maintenance::supervise;
    install();
    // Supervise each domain worker separately: one panic must not stop the others. The
    // supervisor names are kept so worker heartbeats and logs stay continuous.
    let pool = state.pool.clone();
    let expense_state = state.clone();
    tokio::join!(
        supervise("sing-box", move || proxy(state.clone())),
        supervise("ddns", {
            let pool = pool.clone();
            move || naming::run(pool.clone())
        }),
        supervise("alicloud", move || operations::cloud::worker::run(
            pool.clone()
        )),
        supervise("expenses", move || costs::run(expense_state.clone()))
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

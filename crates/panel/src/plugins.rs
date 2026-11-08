//! Compiled domain crates (ADR 0128): their routes and background workers, and the
//! public Rust paths kept for existing callers. The typed interfaces between owners
//! are connected in `wiring`.

pub use apsp_cloud_api as cloud_api;
pub use apsp_costs as costs;
// Previous public names of the costs and naming crates; `apsp_panel::*` paths stay valid.
pub use apsp_costs as expenses;
pub use apsp_naming as ddns;
pub use apsp_naming as naming;
pub use apsp_panel_host::diagnostics::activity::ActivityEvidence;
pub use apsp_plugin_alicloud as alicloud;
pub use apsp_plugin_singbox as singbox;

use std::collections::BTreeMap;

use apsp_panel_host::{AppState, error::ApiResult};
use apsp_protocol::{ModuleManifest, UsageBatch};
use axum::Router;

/// Connects the typed interfaces of the compiled domains; repeated calls are harmless.
pub fn install() {
    crate::wiring::install();
}

pub fn router() -> Router<AppState> {
    install();
    singbox::router()
        .merge(naming::routes())
        .merge(costs::routes())
        .merge(alicloud::routes())
        .merge(alicloud::security_groups_routes())
}

pub async fn run(state: AppState) {
    use apsp_panel_host::maintenance::supervise;
    install();
    // Supervise each domain worker separately: one panic must not stop the others. The
    // supervisor names are kept so worker heartbeats and logs stay continuous.
    let pool = state.pool.clone();
    let expense_state = state.clone();
    tokio::join!(
        supervise("sing-box", move || singbox::run(state.clone())),
        supervise("ddns", {
            let pool = pool.clone();
            move || naming::run(pool.clone())
        }),
        supervise("alicloud", move || alicloud::run(pool.clone())),
        supervise("expenses", move || costs::run(expense_state.clone()))
    );
}

// Direct entry points kept for embedders of the previous public API.

pub async fn runtime_activity_on(
    connection: &mut sqlx::PgConnection,
    server_id: i64,
    checked_at: i64,
) -> ApiResult<ActivityEvidence> {
    crate::wiring::runtime_activity_on(connection, server_id, checked_at).await
}

pub async fn ingest_usage(
    state: &AppState,
    server_id: i64,
    batch: UsageBatch,
) -> anyhow::Result<()> {
    singbox::usage::ingest(state, server_id, batch).await
}

pub async fn manifest_modules(
    state: &AppState,
    server_id: i64,
    info: &serde_json::Value,
) -> ApiResult<BTreeMap<String, ModuleManifest>> {
    crate::wiring::manifest_modules(state, server_id, info).await
}

pub async fn bundle(state: &AppState, server_id: i64, rev: i64) -> ApiResult<String> {
    singbox::agent::bundle(state, server_id, rev).await
}

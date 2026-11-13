#![forbid(unsafe_code)]
//! Traffic service (ADR 0128): nodes, chains and external sources, the configuration
//! each server runs and its publication, runtime control and deployment checks.
//!
//! It reaches panel services through the `apsp-panel-host` API. What it needs from
//! proxy-user access (users' limits, Nexus device limits, the operation log) comes
//! through `users`, which access answers.

/// The signed stable runtime with APSP per-user budgets (ADR 0124).
pub const RUNTIME_VERSION: &str = "1.14.4-apsp.1";

mod activity;
pub mod agent;
pub mod business;
pub mod catalog;
pub mod china_reachability;
pub mod deployments;
pub mod device_config;
pub mod nexus_nodes;
pub mod node_protocol;
mod node_settings;
pub mod nodes;
pub mod ordered_paths;
pub mod proxy_resources;
pub mod publisher;
pub mod runtime_control;
mod runtime_controls;
pub mod runtime_operations;
pub mod runtime_validations;
pub mod runtime_workflows;
pub mod server_profiles;
pub mod settings;
mod subscription_fetch;
pub mod subscription_parser;
pub mod subscription_sources;
pub mod users;

pub use activity::{ActivityEvidence, runtime_activity_on};

pub(crate) use apsp_governance::{artifacts, auth, control_center, releases};
#[cfg(test)]
pub(crate) use apsp_panel_host::config;
pub(crate) use apsp_panel_host::{AppState, device_work, digest, error};
pub(crate) use apsp_resources::device_channel;
use axum::{
    Router,
    routing::{get, post},
};

/// A read-only transaction over one consistent snapshot, started with a single
/// statement instead of BEGIN followed by SET TRANSACTION.
pub async fn read_snapshot(
    pool: &sqlx::PgPool,
) -> sqlx::Result<sqlx::Transaction<'static, sqlx::Postgres>> {
    pool.begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .await
}

/// Refreshes external sources; the publication cycle itself is composed by the
/// application together with access (see `publisher`).
pub async fn run(state: AppState) {
    subscription_sources::worker::run(state).await;
}

/// The domain's routes: the console API and what each device asks.
pub fn routes() -> Router<AppState> {
    management().merge(device_routes())
}

/// The console API for nodes, chains, external sources, server runtimes and their
/// deployment.
fn management() -> Router<AppState> {
    Router::new()
        .route(
            "/api/traffic/servers/{id}/profile",
            get(server_profiles::get).put(server_profiles::put),
        )
        .route(
            "/api/traffic/chains/ordered-batch",
            post(ordered_paths::create_batch),
        )
        .route(
            "/api/traffic/ordered-proxy-resources",
            get(proxy_resources::list),
        )
        .route(
            "/api/traffic/ordered-proxy-resources/{kind}/{id}",
            get(proxy_resources::get)
                .patch(ordered_paths::update_resource)
                .delete(proxy_resources::remove),
        )
        .route(
            "/api/traffic/ordered-proxy-resources/chain/{id}/apply-node-versions",
            post(ordered_paths::apply_node_versions),
        )
        .route("/api/traffic/servers", get(settings::list))
        .route("/api/traffic/servers/{id}", get(settings::get))
        .route("/api/traffic/servers/{id}/enable", post(settings::enable))
        .route(
            "/api/traffic/servers/{id}/deployments",
            get(deployments::get),
        )
        .route(
            "/api/traffic/servers/{id}/deployments/check",
            post(deployments::check),
        )
        .route(
            "/api/traffic/servers/{id}/deployments/redeploy",
            post(deployments::redeploy),
        )
        .route(
            "/api/traffic/servers/{id}/runtime-operations",
            get(runtime_operations::list).post(runtime_operations::create),
        )
        .route("/api/traffic/nodes", get(nodes::list).post(nodes::create))
        .route(
            "/api/traffic/nodes/{id}",
            get(nodes::get).patch(nodes::update).delete(nodes::remove),
        )
        .merge(catalog::router())
        .merge(nexus_nodes::router())
        .merge(runtime_workflows::router())
        .merge(subscription_sources::routes())
}

/// What each device runs and confirms: manifests, bundles, runtime checkpoints and
/// runtime validations.
fn device_routes() -> Router<AppState> {
    Router::new()
        .merge(runtime_control::routes())
        .route("/api/agent/v1/manifest", get(device_config::manifest))
        .route("/api/agent/v1/bundles/{rev}", get(device_config::bundle))
        .route(
            "/api/agent/v1/runtime-validations",
            get(runtime_validations::pending),
        )
        .route(
            "/api/agent/v1/runtime-validations/{id}/result",
            post(runtime_validations::complete),
        )
}

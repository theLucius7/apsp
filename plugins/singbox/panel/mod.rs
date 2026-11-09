#![forbid(unsafe_code)]
//! sing-box business plugin: proxy users, nodes, chains, sources and publication.
//!
//! The plugin reaches panel services only through the `apsp-panel-host` API.

/// The signed stable runtime with APSP per-user budgets (ADR 0124).
pub const RUNTIME_VERSION: &str = "1.14.4-apsp.1";

pub mod accesses;
mod activity;
pub mod agent;
pub mod business;
pub mod catalog;
pub mod chains;
pub mod deployments;
pub mod device_config;
pub mod entitlements;
pub mod external_access;
pub mod host_support;
pub mod legacy_takeover;
pub mod mixed_conversion;
pub mod mixed_paths;
mod nexus_nodes;
mod user_accounts;

/// Records a device's admitted session addresses for Nexus device limits.
pub async fn ingest_runtime_sessions(
    state: &AppState,
    server_id: i64,
    report: apsp_protocol::RuntimeSessionReport,
) -> anyhow::Result<()> {
    nexus_nodes::sessions::ingest(state, server_id, report).await
}

/// The device limit policy the publisher would send to this host now.
pub async fn nexus_admission_policy(
    state: &AppState,
    server_id: i64,
) -> anyhow::Result<Option<apsp_protocol::RuntimeAdmissionPolicy>> {
    let mut tx = state.pool.begin().await?;
    let policy =
        nexus_nodes::sessions::policy(&mut tx, server_id, apsp_protocol::now_timestamp()).await?;
    tx.commit().await?;
    Ok(policy)
}
mod node_protocol;
mod node_settings;
pub mod nodes;
pub mod operations_workflows;
pub mod ordered_paths;
pub mod packages;
pub mod policies;
mod portal;
pub mod proxy_resources;
pub mod proxy_users;
pub mod publisher;
pub mod reachability;
pub mod runtime_control;
mod runtime_controls;
pub mod runtime_operations;
pub mod runtime_validations;
mod secret_access;
pub mod server_profiles;
pub mod settings;
pub mod source_migration;
pub mod sources;
mod subscription_fetch;
pub mod subscription_hosts;
pub mod subscription_parser;
pub mod subscription_sources;
pub mod subscriptions;
pub mod usage;
pub mod user_controls;
pub mod user_limits;
pub mod user_sessions;

pub use activity::{ActivityEvidence, runtime_activity_on};
pub use portal::{HostNetwork, install_host_network};

#[cfg(test)]
pub(crate) use apsp_panel_host::config;
pub(crate) use apsp_panel_host::{
    AppState, agent_api, artifacts, auth, control_center, device_work, error,
    network_configuration, passkeys, releases,
};
use axum::{
    Router,
    routing::{delete, get, post},
};

/// A read-only transaction over one consistent snapshot, started with a single
/// statement instead of BEGIN followed by SET TRANSACTION.
pub(crate) async fn read_snapshot(
    pool: &sqlx::PgPool,
) -> sqlx::Result<sqlx::Transaction<'static, sqlx::Postgres>> {
    pool.begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .await
}

pub async fn run(state: AppState) {
    tokio::join!(
        publisher::run(state.clone()),
        user_accounts::run(state.clone()),
        reachability::run(state.clone()),
        subscription_hosts::run(state.clone()),
        subscription_sources::worker::run(state)
    );
}

pub fn router() -> Router<AppState> {
    let management = Router::new()
        .route(
            "/servers/{id}/profile",
            get(server_profiles::get).put(server_profiles::put),
        )
        .route("/policy-groups", get(policies::list).post(policies::create))
        .route(
            "/policy-groups/{id}",
            axum::routing::put(policies::update).delete(policies::remove),
        )
        .route(
            "/package-groups",
            get(packages::list).post(packages::create),
        )
        .route(
            "/package-groups/{id}",
            axum::routing::put(packages::update).delete(packages::remove),
        )
        .route("/chains", get(chains::list).post(chains::create))
        .route("/chains/ordered-batch", post(ordered_paths::create_batch))
        .route("/chains/{id}", delete(chains::remove))
        .route(
            "/proxy-resources/chain/{id}/conversion",
            get(mixed_conversion::precheck).post(mixed_conversion::convert),
        )
        .route("/ordered-proxy-resources", get(proxy_resources::list))
        .route(
            "/ordered-proxy-resources/{kind}/{id}",
            get(proxy_resources::get)
                .patch(ordered_paths::update_resource)
                .delete(proxy_resources::remove),
        )
        .route(
            "/ordered-proxy-resources/chain/{id}/apply-node-versions",
            post(ordered_paths::apply_node_versions),
        )
        .route(
            "/users/{id}/policy-groups",
            get(policies::user_get).put(policies::user_set),
        )
        .route("/users/{id}/package", post(packages::assign))
        .route("/users/{id}/entitlement", get(entitlements::get))
        .route(
            "/users/{id}/limits",
            get(user_limits::get).put(user_limits::put),
        )
        .route("/users/{id}/sessions", get(user_sessions::get))
        .route(
            "/users/{id}/sessions/{server}",
            post(user_sessions::request),
        )
        .route(
            "/users/{id}/control",
            get(user_controls::get).put(user_controls::put),
        )
        .route("/servers", get(settings::list))
        .route("/servers/{id}", get(settings::get))
        .route("/servers/{id}/enable", post(settings::enable))
        .route("/servers/{id}/deployments", get(deployments::get))
        .route("/servers/{id}/deployments/check", post(deployments::check))
        .route(
            "/servers/{id}/deployments/redeploy",
            post(deployments::redeploy),
        )
        .route(
            "/servers/{id}/runtime-operations",
            get(runtime_operations::list).post(runtime_operations::create),
        )
        .route("/nodes", get(nodes::list).post(nodes::create))
        .route(
            "/nodes/{id}",
            get(nodes::get).patch(nodes::update).delete(nodes::remove),
        )
        .route("/users", get(proxy_users::list).post(proxy_users::create))
        .route("/users/overview", get(proxy_users::overview))
        .route(
            "/subscription-domain",
            get(subscription_hosts::get_settings).put(subscription_hosts::put_settings),
        )
        .route(
            "/users/{id}",
            get(proxy_users::get)
                .patch(proxy_users::update)
                .delete(proxy_users::remove),
        )
        .route(
            "/users/{id}/subscription/reset",
            post(proxy_users::reset_subscription),
        )
        .route("/users/{id}/subscription", get(subscriptions::preview))
        .route("/users/{id}/accesses", get(accesses::list))
        .route(
            "/external-node-candidates",
            get(external_access::candidates),
        )
        .route("/usage", get(usage::summary))
        .merge(catalog::router())
        .merge(nexus_nodes::router())
        .merge(reachability::router())
        .merge(sources::router())
        .route("/source-migration", get(source_migration::state))
        .merge(portal::router())
        .merge(user_accounts::router())
        .merge(mixed_paths::router());
    let management = management.merge(operations_workflows::router());
    Router::new()
        .nest(
            "/api/plugins/sing-box",
            management.merge(subscription_sources::routes()),
        )
        .route("/sub/{token}", get(subscriptions::get))
        // What each device runs and confirms: manifests, bundles, runtime checkpoints and
        // runtime validations.
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

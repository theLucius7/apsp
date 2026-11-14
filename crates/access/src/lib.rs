#![forbid(unsafe_code)]
//! Proxy-user access (ADR 0128): proxy users, policy groups and packages, entitlements
//! and quotas, subscriptions and personal addresses, the member portal, user limits and
//! the proxy operation workflows.
//!
//! It builds on the traffic service's nodes, chains and external nodes, and answers the
//! interface traffic declares for users' limits, device limits and the operation log.

pub mod accesses;
pub mod entitlements;
pub mod external_access;
pub mod locks;
pub mod nexus;
pub mod operations_workflows;
pub mod packages;
pub mod policies;
mod portal;
pub mod proxy_users;
mod secret_access;
pub mod subscription_hosts;
pub mod subscriptions;
pub mod usage;
mod user_accounts;
pub mod user_controls;
pub mod user_limits;
pub mod user_sessions;

pub use portal::{HostNetwork, install_host_network};

pub(crate) use apsp_governance::{auth, control_center, passkeys};
pub(crate) use apsp_panel_host::{AppState, device_work, digest, error};
pub(crate) use apsp_resources::{device_channel, retirement, server_observation, servers};
use axum::{
    Router,
    routing::{get, post},
};

/// The message queues access keeps, each drained by the platform's deliverer.
pub const QUEUES: &[apsp_panel_host::notifications::queue::Policy] = &[user_accounts::QUEUE];

/// Member accounts and personal subscription hosts; the publication cycle that refreshes
/// entitlements is composed by the application together with traffic.
pub async fn run(state: AppState) {
    tokio::join!(
        user_accounts::run(state.clone()),
        subscription_hosts::run(state)
    );
}

/// The domain's routes: the console API, the member portal and the subscription address.
pub fn routes() -> Router<AppState> {
    management().merge(subscription_routes())
}

/// The console API for proxy users, policies, packages, subscriptions, the member portal
/// and operation workflows.
fn management() -> Router<AppState> {
    Router::new()
        .route(
            "/api/access/policy-groups",
            get(policies::list).post(policies::create),
        )
        .route(
            "/api/access/policy-groups/{id}",
            axum::routing::put(policies::update).delete(policies::remove),
        )
        .route(
            "/api/access/package-groups",
            get(packages::list).post(packages::create),
        )
        .route(
            "/api/access/package-groups/{id}",
            axum::routing::put(packages::update).delete(packages::remove),
        )
        .route(
            "/api/access/users/{id}/policy-groups",
            get(policies::user_get).put(policies::user_set),
        )
        .route("/api/access/users/{id}/package", post(packages::assign))
        .route("/api/access/users/{id}/entitlement", get(entitlements::get))
        .route(
            "/api/access/users/{id}/limits",
            get(user_limits::get).put(user_limits::put),
        )
        .route("/api/access/users/{id}/sessions", get(user_sessions::get))
        .route(
            "/api/access/users/{id}/sessions/{server}",
            post(user_sessions::request),
        )
        .route(
            "/api/access/users/{id}/control",
            get(user_controls::get).put(user_controls::put),
        )
        .route(
            "/api/access/users",
            get(proxy_users::list).post(proxy_users::create),
        )
        .route("/api/access/users/overview", get(proxy_users::overview))
        .route(
            "/api/access/subscription-domain",
            get(subscription_hosts::get_settings).put(subscription_hosts::put_settings),
        )
        .route(
            "/api/access/users/{id}",
            get(proxy_users::get)
                .patch(proxy_users::update)
                .delete(proxy_users::remove),
        )
        .route(
            "/api/access/users/{id}/subscription/reset",
            post(proxy_users::reset_subscription),
        )
        .route(
            "/api/access/users/{id}/subscription",
            get(subscriptions::preview),
        )
        .route("/api/access/users/{id}/accesses", get(accesses::list))
        .route(
            "/api/access/external-node-candidates",
            get(external_access::candidates),
        )
        .route("/api/access/usage", get(usage::summary))
        .merge(portal::router())
        .merge(user_accounts::router())
        .merge(operations_workflows::router())
}

/// The subscription address proxy clients use.
fn subscription_routes() -> Router<AppState> {
    Router::new().route("/sub/{token}", get(subscriptions::get))
}

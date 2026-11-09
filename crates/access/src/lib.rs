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
pub mod nexus_sessions;
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

pub(crate) use apsp_panel_host::{
    AppState, agent_api, auth, control_center, device_work, error, passkeys,
};
use axum::{
    Router,
    routing::{get, post},
};

/// Member accounts and personal subscription hosts; the publication cycle that refreshes
/// entitlements is composed by the application together with traffic.
pub async fn run(state: AppState) {
    tokio::join!(
        user_accounts::run(state.clone()),
        subscription_hosts::run(state)
    );
}

/// The management API for proxy users, policies, packages, subscriptions, the member
/// portal and operation workflows, relative to the console prefix the application nests
/// it under.
pub fn management() -> Router<AppState> {
    Router::new()
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
        .merge(portal::router())
        .merge(user_accounts::router())
        .merge(operations_workflows::router())
}

/// The subscription address proxy clients use.
pub fn subscription_routes() -> Router<AppState> {
    Router::new().route("/sub/{token}", get(subscriptions::get))
}

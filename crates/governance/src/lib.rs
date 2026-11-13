#![forbid(unsafe_code)]
//! Platform governance (ADR 0128): who may administer the panel and how the panel itself
//! stays trustworthy. Administrators with their sessions, sign-in factors and Passkeys,
//! the request guard with its permissions and audit trail, managed credentials, panel
//! settings, and the signed releases and artifacts the panel trusts.

pub mod artifacts;
pub mod auth;
pub mod control_center;
pub mod passkeys;
pub mod releases;
mod routes;

pub use routes::routes;

pub(crate) use apsp_panel_host::{AppState, error, notifications, ports, settings, tokens};

/// The shared state with governance prepared: the first administrator exists and the
/// Passkey service is built for the panel's public address.
pub async fn state(
    pool: sqlx::PgPool,
    config: apsp_panel_host::config::Config,
) -> anyhow::Result<AppState> {
    let state = AppState::new(pool, config).await?;
    auth::ensure_admin(&state.pool, state.config.admin_password.as_deref()).await?;
    passkeys::service(&state);
    Ok(state)
}

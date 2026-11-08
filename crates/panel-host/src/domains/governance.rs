//! Platform governance: administrators and their sessions, panel settings,
//! signed artifacts and Agent versions, audit and the control center.

use axum::{
    Router,
    routing::{get, post},
};

use crate::{
    AppState, agent_updates, artifacts, auth,
    control_center::{self, panel_settings},
    releases,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(auth::passkeys::routes())
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route(
            "/api/settings",
            get(panel_settings::get).patch(panel_settings::update),
        )
        .route("/api/notifications/channels", get(panel_settings::channels))
        .route(
            "/api/notifications/webhook",
            get(panel_settings::webhook)
                .patch(panel_settings::update_webhook)
                .delete(panel_settings::remove_webhook),
        )
        .route(
            "/api/notifications/webhook/test",
            post(panel_settings::test_webhook),
        )
        .route(
            "/api/notifications/telegram/test",
            post(panel_settings::test_telegram),
        )
        .route("/api/security/totp", get(auth::totp_status))
        .route("/api/security/totp/setup", post(auth::totp_setup))
        .route("/api/security/totp/confirm", post(auth::totp_confirm))
        .route("/api/security/totp/disable", post(auth::totp_disable))
        .route("/api/security/password", post(auth::password_change))
        .merge(control_center::router())
        .route("/api/artifacts", get(artifacts::list))
        .route("/api/artifacts/targets", get(releases::target_options))
        .route(
            "/api/artifacts/agent-versions",
            get(releases::list_agent_versions),
        )
        .route(
            "/api/bootstrap/versions",
            get(releases::bootstrap_agent_versions),
        )
        .route("/api/artifacts/import-release", post(releases::import))
        .route("/api/bootstrap/{version}/{arch}", get(artifacts::bootstrap))
        .route("/install.sh", get(artifacts::install_script))
        .route("/install.ps1", get(artifacts::install_powershell))
        .route("/api/agent/v1/update", get(agent_updates::available))
        .route(
            "/api/agent/v1/artifacts/{name}/{version}/{arch}",
            get(artifacts::download),
        )
}

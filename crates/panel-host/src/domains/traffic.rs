//! Traffic services whose code still lives in this crate: what each device should
//! run, the revisions it applied, runtime checkpoints and runtime validations.

use axum::{
    Router,
    routing::{get, post},
};

use crate::{AppState, device_config, runtime_control, runtime_validations};

pub fn routes() -> Router<AppState> {
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

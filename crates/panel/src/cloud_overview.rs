//! The cloud page's combined view: accounts and resources with their bills and power
//! settings, bandwidth operations, power jobs, address rotations and notifications. It
//! joins several domains, so it belongs to the application (ADR 0128).

use crate::{AppState, auth, error::ApiResult};
use apsp_observability::cloud_notices;
use apsp_operations::cloud::{bandwidth, eip_rotation, power};
use apsp_resources::cloud::inventory;
use axum::{Json, Router, extract::State, http::HeaderMap, routing::get};
use serde_json::{Value, json};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/plugins/alicloud", get(list))
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    let (accounts, resources) = inventory::listed(&state.pool).await?;
    let operations = bandwidth::recent(&state.pool).await?;
    let power_jobs = power::recent(&state.pool).await?;
    let events = cloud_notices::recent(&state.pool).await?;
    let eip_rotations = eip_rotation::recent(&state.pool).await?;
    Ok(Json(
        json!({"accounts":accounts,"resources":resources,"operations":operations,"power_jobs":power_jobs,"eip_rotations":eip_rotations,"events":events}),
    ))
}

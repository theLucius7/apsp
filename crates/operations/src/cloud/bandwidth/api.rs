//! Bandwidth and charge changes: preview, confirmation, cancellation and dismissal.

use crate::{AppState, auth, error::ApiResult};
use apsp_resources::cloud::{
    client::Cloud,
    failure,
    model::{Operation, Target},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::Deserialize;
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/resources/alicloud/resources/{id}/preview",
            post(preview),
        )
        .route(
            "/api/resources/alicloud/operations/{id}/confirm",
            post(confirm),
        )
        .route(
            "/api/resources/alicloud/operations/{id}/cancel",
            post(cancel),
        )
        .route(
            "/api/resources/alicloud/operations/{id}/dismiss",
            post(dismiss),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    target: Target,
    revision: i64,
}
async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<Preview>,
) -> ApiResult<Json<Operation>> {
    auth::require_admin(&state, &headers).await?;
    Ok(Json(
        super::preview(
            &state.pool,
            id,
            input.target,
            input.revision,
            &Cloud::new(&state.pool).map_err(failure)?,
        )
        .await?,
    ))
}
async fn confirm(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<(StatusCode, Json<Operation>)> {
    auth::require_admin(&state, &headers).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(super::confirm(&state.pool, id).await?),
    ))
}
async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    auth::require_admin(&state, &headers).await?;
    super::cancel(&state.pool, id, false).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn dismiss(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    auth::require_admin(&state, &headers).await?;
    super::cancel(&state.pool, id, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

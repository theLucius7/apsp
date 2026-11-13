//! Administrators read and change the panel settings and the notification channels
//! here; the settings store and the channels themselves belong to the platform.

use axum::{Json, extract::State, http::HeaderMap};
use serde_json::Value;

use crate::{
    AppState, auth,
    error::ApiResult,
    notifications::{self, webhook_settings},
    settings,
};

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    settings::current_view(&state).await
}

pub async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(update): Json<settings::Update>,
) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    settings::apply(&state, update).await
}

pub async fn channels(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    notifications::channels::status(&state).await
}

pub async fn test_telegram(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    notifications::channels::test(&state, "telegram").await
}

pub async fn test_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    notifications::channels::test(&state, "webhook").await
}

pub async fn webhook(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    webhook_settings::current(&state).await
}

pub async fn update_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(update): Json<webhook_settings::Update>,
) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    webhook_settings::apply(&state, update).await
}

pub async fn remove_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    webhook_settings::clear(&state).await
}

//! What an installing machine asks with its enrollment token: the install scripts and the
//! Agent versions it may choose. Binaries are not served; the trusted installer fetches
//! them from the signed release.

use crate::{
    AppState,
    error::{ApiError, ApiResult},
    installation, releases, servers,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct TokenQuery {
    pub token: String,
    pub agent_version: Option<String>,
    pub agent_target: Option<String>,
    pub platform: Option<String>,
}

pub async fn bootstrap_agent_versions(
    State(state): State<AppState>,
    Query(query): Query<releases::BootstrapVersionQuery>,
) -> ApiResult<Response> {
    servers::validate_enrollment(&state.pool, &query.token).await?;
    releases::installable_agent_versions(&state, &query).await
}

pub async fn bootstrap(
    State(state): State<AppState>,
    Query(query): Query<TokenQuery>,
    Path((_version, _arch)): Path<(String, String)>,
) -> ApiResult<Response> {
    servers::validate_enrollment(&state.pool, &query.token).await?;
    Err(ApiError::Conflict(
        "Agent 请使用从 GitHub 下载的可信安装器，面板不再提供二进制".into(),
    ))
}

pub async fn install_script(
    State(state): State<AppState>,
    Query(query): Query<TokenQuery>,
) -> ApiResult<Response> {
    servers::validate_enrollment(&state.pool, &query.token).await?;
    let installation = installation::select(
        &state,
        query.agent_version.as_deref(),
        &query.token,
        query.platform.as_deref(),
        query.agent_target.as_deref(),
    )
    .await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(installation)).into_response())
}

pub async fn install_powershell(
    State(state): State<AppState>,
    Query(query): Query<TokenQuery>,
) -> ApiResult<Response> {
    servers::validate_enrollment(&state.pool, &query.token).await?;
    let installation = installation::select(
        &state,
        query.agent_version.as_deref(),
        &query.token,
        Some("windows"),
        query.agent_target.as_deref(),
    )
    .await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(installation)).into_response())
}

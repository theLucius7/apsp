//! What a connected device downloads from the panel: runtime and tool artifacts of the
//! imported signed releases. The Agent binary itself comes from GitHub, never from here.

use crate::{
    AppState, device_channel,
    error::{ApiError, ApiResult},
    releases,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};

pub async fn download(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((name, version, arch)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let server_id = device_channel::require_agent(&state, &headers).await?;
    device_channel::require_signed_agent(&state, server_id).await?;
    if name == "agent" {
        return Err(ApiError::Conflict(
            "Agent 请从 GitHub Release 下载，面板不再提供二进制".into(),
        ));
    }
    let (bytes, _, _) = releases::artifact(&state, &name, &version, &arch).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response())
}

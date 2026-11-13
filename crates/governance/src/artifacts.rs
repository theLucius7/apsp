use crate::{AppState, auth, error::ApiResult};
use apsp_protocol::Artifact;
use axum::{Json, extract::State, http::HeaderMap};
use serde::Serialize;

#[derive(Serialize)]
pub struct ArtifactEntry {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub sha256: String,
    pub bytes: u64,
}

pub async fn descriptor(
    state: &AppState,
    name: &str,
    version: &str,
    arch: &str,
) -> ApiResult<Artifact> {
    let (sha256, proof) = crate::releases::artifact_digest(state, name, version, arch).await?;
    Ok(Artifact {
        url: format!(
            "{}/api/agent/v1/artifacts/{name}/{version}/{arch}",
            state.config.public_url
        ),
        sha256,
        proof: Some(proof),
    })
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<ArtifactEntry>>> {
    auth::require_admin(&state, &headers).await?;
    Ok(Json(crate::releases::entries(&state).await?))
}

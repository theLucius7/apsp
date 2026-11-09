//! Read-only view of a user's node credentials. Access itself is granted only
//! through policy groups (ADR 0112).
use crate::{
    AppState,
    auth::require_admin,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Serialize, FromRow)]
pub struct AccessView {
    pub user_id: i64,
    pub node_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uuid: Option<Uuid>,
    pub stat_name: String,
}
impl AccessView {
    fn visibility(mut self, reveal: bool) -> Self {
        if !reveal {
            self.uuid = None;
        }
        self
    }
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<AccessView>>> {
    require_admin(&state, &headers).await?;
    let reveal = super::secret_access::may_reveal(&state, &headers).await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND deleted_at IS NULL)")
            .bind(id)
            .fetch_one(&state.pool)
            .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    let accesses = sqlx::query_as::<_, AccessView>("SELECT a.user_id,a.node_id,a.uuid,a.stat_name FROM accesses a JOIN nodes n ON n.id=a.node_id JOIN servers s ON s.id=n.server_id WHERE a.user_id=$1 AND n.deleted_at IS NULL AND s.deleted_at IS NULL ORDER BY a.node_id").bind(id).fetch_all(&state.pool).await?;
    if reveal {
        super::secret_access::audit_read(&state,&headers,Some(id),"security_node_credentials_read",serde_json::json!({"source":"access_list","node_ids":accesses.iter().map(|access|access.node_id).collect::<Vec<_>>(),"credential_values_recorded":false})).await?;
    }
    Ok(Json(
        accesses
            .into_iter()
            .map(|access| access.visibility(reveal))
            .collect(),
    ))
}

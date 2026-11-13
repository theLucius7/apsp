use crate::{AppState, auth, error::ApiResult};
use axum::{Json, Router, extract::State, http::HeaderMap, routing::get};
use serde::Serialize;
use sqlx::FromRow;

/// A server a rule can take its address from. Every server with an Agent can
/// be chosen: DNS updates run in the panel, so there is nothing to switch on
/// per server (ADR 0110).
#[derive(Serialize, FromRow)]
struct Server {
    id: i64,
    name: String,
    online: bool,
    interface_names: Vec<String>,
    public_discovery_available: bool,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/api/naming/servers", get(list))
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Vec<Server>>> {
    auth::require_admin(&state, &headers).await?;
    crate::control_center::require_capability(&state, &headers, "dns:read").await?;
    let actor = crate::control_center::authenticate(&state, &headers).await?;
    let servers: Vec<Server> = sqlx::query_as("SELECT s.id,s.name,COALESCE(s.last_seen>=$1-60,false) AS online,ARRAY(SELECT jsonb_object_keys(CASE WHEN jsonb_typeof(s.static_info->'interface_addresses')='object' THEN s.static_info->'interface_addresses' ELSE '{}'::jsonb END)) AS interface_names,COALESCE(jsonb_typeof(s.static_info->'discovered_public_ips')='array',false) AS public_discovery_available FROM servers s WHERE s.deleted_at IS NULL ORDER BY s.id")
        .bind(apsp_protocol::now_timestamp()).fetch_all(&state.pool).await?;
    Ok(Json(
        servers
            .into_iter()
            .filter(|server| actor.allows_server(server.id))
            .collect(),
    ))
}

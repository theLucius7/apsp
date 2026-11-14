//! The servers an administrator may see, with their system facts and newest metrics, for
//! management tools. It joins resources' servers with the metrics observability keeps, so
//! it belongs to the application (ADR 0128).

use apsp_governance::control_center::{authenticate, require_capability};
use apsp_panel_host::{AppState, error::ApiResult};
use apsp_protocol::now_timestamp;
use axum::{Json, extract::State, http::HeaderMap};
use serde_json::{Value, json};
use sqlx::Row;

pub async fn export_servers(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let actor = authenticate(&state, &headers).await?;
    require_capability(&state, &headers, "servers:read").await?;
    let rows = sqlx::query("SELECT id,name,last_seen,manifest_rev,static_info FROM servers WHERE deleted_at IS NULL ORDER BY id LIMIT 10000")
        .fetch_all(&state.pool)
        .await?;
    let mut allowed = Vec::new();
    for row in rows {
        if actor.allows_server(row.try_get("id")?) {
            allowed.push(row);
        }
    }
    let ids = allowed
        .iter()
        .map(|row| row.try_get("id"))
        .collect::<Result<Vec<i64>, _>>()?;
    let mut metrics =
        apsp_observability::telemetry::latest_metrics(&mut *state.pool.acquire().await?, &ids)
            .await?;
    let mut values = Vec::new();
    for row in allowed {
        let id: i64 = row.try_get("id")?;
        values.push(json!({"id":id,"name":row.try_get::<String,_>("name")?,"last_seen":row.try_get::<Option<i64>,_>("last_seen")?,"manifest_rev":row.try_get::<i64,_>("manifest_rev")?,"static_info":row.try_get::<Value,_>("static_info")?,"metrics":metrics.remove(&id).unwrap_or_default().metrics}));
    }
    Ok(Json(
        json!({"schema_version":1,"exported_at":now_timestamp(),"scope":"authorized-servers","servers":values,"credentials_included":false}),
    ))
}

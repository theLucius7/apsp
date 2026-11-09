use super::super::{
    business::{NODE_COLUMNS, NodeRow, NodeView},
    nodes::CreateNode,
};
use crate::{
    AppState,
    auth::require_admin,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloneNode {
    revision: String,
    server_id: i64,
    name: String,
    port: Option<i64>,
    public_host: Option<String>,
    sni: Option<String>,
}

pub async fn clone_node(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(request): Json<CloneNode>,
) -> ApiResult<(StatusCode, Json<NodeView>)> {
    require_admin(&state, &headers).await?;
    let mut tx = state.pool.begin().await?;
    crate::business::lock_publication(&mut tx).await?;
    let resource = super::projection::catalog_on(&mut tx)
        .await?
        .into_iter()
        .find(|resource| resource["kind"] == "direct" && resource["id"] == id)
        .ok_or(ApiError::NotFound)?;
    if resource["revision"] != request.revision {
        return Err(ApiError::Conflict(format!(
            "节点 direct:{id} 已发生变化，请刷新后重新确认复制"
        )));
    }
    let query = format!(
        "SELECT {NODE_COLUMNS} FROM nodes n JOIN servers s ON s.id=n.server_id WHERE n.id=$1 AND n.deleted_at IS NULL AND s.deleted_at IS NULL"
    );
    let source = sqlx::query_as::<_, NodeRow>(&query)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let mut protocol = source.protocol_config;
    if let Some(object) = protocol.as_object_mut() {
        object.remove("password");
        object.remove("psk");
    }
    // Reusing the creation path regenerates runtime identities and obfuscation
    // secrets. Access grants are deliberately absent from the copied model.
    let settings = super::super::node_settings::view(source.settings)?;
    let created = super::super::nodes::create_on(
        &mut tx,
        CreateNode {
            enabled: Some(source.enabled),
            settings: serde_json::from_value(settings).map_err(anyhow::Error::from)?,
            name: request.name,
            server_id: request.server_id,
            public_host: request.public_host.unwrap_or(source.public_host),
            sni: request.sni.unwrap_or(source.sni),
            protocol_config: serde_json::from_value(protocol).map_err(anyhow::Error::from)?,
            port: request.port,
        },
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(created)))
}

//! Compose the usage trends endpoint with the access domain's ledger summary.

use super::StatisticsQuery;
use crate::{AppState, auth, error::ApiResult};
use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use serde_json::Value;

pub async fn summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<StatisticsQuery>,
) -> ApiResult<Json<Value>> {
    auth::require_admin(&state, &headers).await?;
    let window = query.window()?;
    Ok(Json(
        apsp_access::usage::statistics::summary(&state.pool, window.days, window.from, window.now)
            .await?,
    ))
}

//! Payment history across the ledger (ADR 0097): payments confirmed for manual
//! items, and the cost records kept for servers in their asset ledger. Server
//! records stay in `fleet_cost_records` and keep their minor-unit amounts; the
//! client formats them with the currency's own decimal places.
use super::*;
use axum::{Json, extract::State};

pub(super) async fn payments(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let principal = principal(&state, &headers, false).await?;
    let items: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('kind','item','id',p.id::text,'subscription_id',p.subscription_id,'name',s.name,\
         'paid_on',p.paid_on::text,'period_start',p.period_start::text,'amount_units',p.amount_units::text,'currency',p.currency) \
         FROM expense_payments p JOIN expense_subscriptions s ON s.id=p.subscription_id \
         ORDER BY p.paid_on DESC,p.id DESC LIMIT 500",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut servers = Vec::new();
    if principal.allows("servers:read") {
        let rows = sqlx::query(
            "SELECT r.server_id,jsonb_build_object('kind','server','id',r.id::text,'server_id',r.server_id,'name',s.name,\
             'record_kind',r.kind,'paid_on',(to_timestamp(r.occurred_at) AT TIME ZONE e.timezone)::date::text,\
             'valid_until',r.valid_until,'amount_minor',r.amount_minor::text,'currency',r.currency,'reference',r.reference) AS value \
             FROM fleet_cost_records r JOIN servers s ON s.id=r.server_id CROSS JOIN expense_settings e \
             WHERE e.singleton AND s.deleted_at IS NULL ORDER BY r.occurred_at DESC,r.id LIMIT 1000",
        )
        .fetch_all(&state.pool)
        .await?;
        for row in rows {
            if principal.allows_server(row.try_get("server_id")?) {
                servers.push(row.try_get::<Value, _>("value")?);
            }
        }
    }
    Ok(Json(json!({"items":items,"servers":servers})))
}

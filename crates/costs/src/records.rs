//! What was paid for a server, recorded in the costs ledger (ADR 0097, ADR 0137 step 2):
//! purchases, renewals, refunds, price changes and planned cancellations, in the currency's
//! minor unit. A purchase or renewal with a validity date moves the server's expiry in its
//! asset settings; the records feed the payment history, the supplier comparison and the
//! cancellation reminders.

use apsp_governance::control_center;
use apsp_panel_host::{
    AppState,
    error::{ApiError, ApiResult},
    evidence, money, text,
};
use apsp_protocol::now_timestamp;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostInput {
    kind: String,
    amount_minor: i64,
    currency: String,
    occurred_at: i64,
    valid_until: Option<i64>,
    reference: String,
}

pub async fn add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<CostInput>,
) -> ApiResult<Json<Value>> {
    control_center::require_server(&state, &headers, id, "servers:write").await?;
    let currency = input.currency.to_ascii_uppercase();
    if ![
        "purchase",
        "renewal",
        "price_change",
        "refund",
        "cancellation",
    ]
    .contains(&input.kind.as_str())
        || !(0..=100_000_000_000).contains(&input.amount_minor)
        || !money::currency(&currency)
        || input.occurred_at < 0
        || input
            .valid_until
            .is_some_and(|until| until < input.occurred_at)
    {
        return Err(ApiError::BadRequest(
            "费用记录参数无效，金额以最小货币单位保存".into(),
        ));
    }
    let updates_expiry = matches!(input.kind.as_str(), "purchase" | "renewal");
    let record_id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO fleet_cost_records(id,server_id,kind,amount_minor,currency,occurred_at,valid_until,reference,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(record_id).bind(id).bind(input.kind).bind(input.amount_minor).bind(currency).bind(input.occurred_at).bind(input.valid_until).bind(text::bounded(&input.reference,512)?).bind(now_timestamp()).execute(&mut *tx).await?;
    if updates_expiry && let Some(until) = input.valid_until {
        apsp_resources::server_assets::record_expiry_tx(&mut tx, id, until).await?;
    }
    evidence::record_event_tx(&mut tx, id, "cost_recorded", json!({"record_id":record_id})).await?;
    tx.commit().await?;
    Ok(Json(json!({"id":record_id})))
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    control_center::require_server(&state, &headers, id, "servers:read").await?;
    let records:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(r)-'server_id' FROM fleet_cost_records r WHERE server_id=$1 ORDER BY occurred_at DESC,id LIMIT 1000").bind(id).fetch_all(&state.pool).await?;
    let annual:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('year',extract(year FROM to_timestamp(occurred_at))::integer,'currency',currency,'amount_minor',sum(CASE WHEN kind='refund' THEN -amount_minor WHEN kind IN ('purchase','renewal') THEN amount_minor ELSE 0 END)::text) FROM fleet_cost_records WHERE server_id=$1 GROUP BY extract(year FROM to_timestamp(occurred_at)),currency ORDER BY extract(year FROM to_timestamp(occurred_at)) DESC,currency").bind(id).fetch_all(&state.pool).await?;
    Ok(Json(
        json!({"records":records,"annual":annual,"currency_conversion":false}),
    ))
}

#![forbid(unsafe_code)]
//! Recurring service expenses, independent of proxy member commerce.
mod api;
mod calendar;
mod history;
mod migration;
pub mod schedule;
mod servers;
mod worker;
pub use worker::tick;

use apsp_panel_host::{
    AppState, control_center,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use axum::{
    Router,
    http::HeaderMap,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

type Tx<'a> = Transaction<'a, Postgres>;
pub fn routes() -> Router<AppState> {
    Router::new().nest(
        "/api/plugins/expenses",
        Router::new()
            .route("/settings", get(api::settings).put(api::settings_save))
            .route("/labels", get(api::labels).post(api::label_create))
            .route(
                "/labels/{id}",
                axum::routing::put(api::label_update).delete(api::label_remove),
            )
            .route("/subscriptions", get(api::list).post(api::create))
            .route(
                "/subscriptions/{id}",
                axum::routing::put(api::update).delete(api::remove),
            )
            .route("/subscriptions/{id}/clone", post(api::clone_entry))
            .route("/subscriptions/{id}/renew", post(api::renew))
            .route("/subscriptions/{id}/payments", get(api::payments))
            .route("/servers", get(servers::list))
            .route("/references", get(servers::reference_list))
            .route("/payments", get(history::payments))
            .route("/calendar", get(calendar::calendar))
            .route("/calendar.ics", get(calendar::ical))
            .route("/summary", get(calendar::summary))
            .route("/notifications", get(worker::notifications))
            .route("/export", get(migration::export))
            .route(
                "/import/preview",
                post(migration::preview)
                    .layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024)),
            )
            .route(
                "/import",
                post(migration::import)
                    .layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024)),
            ),
    )
}
pub async fn run(state: AppState) {
    worker::run(state).await;
}
fn bad(message: &str) -> ApiError {
    ApiError::BadRequest(message.into())
}
fn conflict(message: &str) -> ApiError {
    ApiError::Conflict(message.into())
}
async fn actor(state: &AppState, headers: &HeaderMap, write: bool) -> ApiResult<i64> {
    control_center::require_capability(
        state,
        headers,
        if write { "cloud:write" } else { "cloud:read" },
    )
    .await
}
/// Like `actor`, but keeps the principal so server items follow its server scope.
async fn principal(
    state: &AppState,
    headers: &HeaderMap,
    write: bool,
) -> ApiResult<control_center::Principal> {
    let principal = control_center::authenticate(state, headers).await?;
    if !principal.allows(if write { "cloud:write" } else { "cloud:read" }) {
        return Err(ApiError::Forbidden(
            "当前管理员或 API 令牌没有此操作权限".into(),
        ));
    }
    Ok(principal)
}
async fn lock(tx: &mut Tx<'_>) -> ApiResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(73201067)")
        .execute(&mut **tx)
        .await?;
    Ok(())
}
fn text(value: &str, max: usize, required: bool) -> ApiResult<String> {
    let value = value.trim();
    if (required && value.is_empty())
        || value.chars().count() > max
        || value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(bad("字段长度或格式无效"));
    }
    Ok(value.into())
}
fn units(value: &str) -> ApiResult<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|n| *n >= 0 && *n <= 9_000_000_000_000_000 && n.to_string() == value)
        .ok_or_else(|| bad("金额须为有效的万分之一货币单位整数"))
}
fn currency(value: &str) -> bool {
    value.len() == 3 && value.bytes().all(|b| b.is_ascii_uppercase())
}
async fn event(tx: &mut Tx<'_>, entry: Option<i64>, kind: &str, detail: Value) -> ApiResult<()> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO expense_events(id,subscription_id,event_key,kind,detail,created_at) VALUES($1,$2,$3,$4,$5,$6)").bind(id).bind(entry).bind(id.to_string()).bind(kind).bind(detail).bind(now_timestamp()).execute(&mut **tx).await?;
    Ok(())
}

#[derive(Clone, Deserialize, Serialize, sqlx::FromRow)]
pub struct Subscription {
    pub id: i64,
    pub name: String,
    pub amount_units: String,
    pub currency: String,
    pub start_date: Option<String>,
    pub next_payment: String,
    pub anchor_payment: String,
    pub cycle: String,
    pub frequency: i32,
    pub category_id: Option<i64>,
    pub payment_method_id: Option<i64>,
    pub payer_id: Option<i64>,
    pub notes: String,
    pub url: String,
    pub inactive: bool,
    pub auto_renew: bool,
    pub notify: bool,
    pub notify_days_before: i32,
    pub cancellation_date: Option<String>,
    pub replacement_id: Option<i64>,
    pub version: i64,
}
const COLUMNS: &str = "id,name,amount_units::text,currency,start_date::text,next_payment::text,anchor_payment::text,cycle,frequency,category_id,payment_method_id,payer_id,notes,url,inactive,auto_renew,notify,notify_days_before,cancellation_date::text,replacement_id,version";
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub name: String,
    pub amount_units: String,
    pub currency: String,
    pub start_date: Option<String>,
    pub next_payment: String,
    pub cycle: String,
    pub frequency: i32,
    pub category_id: Option<i64>,
    pub payment_method_id: Option<i64>,
    pub payer_id: Option<i64>,
    pub notes: String,
    pub url: String,
    pub inactive: bool,
    pub auto_renew: bool,
    pub notify: bool,
    pub notify_days_before: i32,
    pub cancellation_date: Option<String>,
    pub replacement_id: Option<i64>,
    pub version: Option<i64>,
}
fn validate(input: &Input) -> ApiResult<()> {
    text(&input.name, 200, true)?;
    units(&input.amount_units)?;
    let next = schedule::parse(&input.next_payment)?;
    let start = input
        .start_date
        .as_deref()
        .map(schedule::parse)
        .transpose()?;
    let cancel = input
        .cancellation_date
        .as_deref()
        .map(schedule::parse)
        .transpose()?;
    if !currency(&input.currency)
        || !(1..=1000).contains(&input.frequency)
        || !["day", "week", "month", "year"].contains(&input.cycle.as_str())
        || !(0..=365).contains(&input.notify_days_before)
        || start.is_some_and(|v| v > next)
        || start.zip(cancel).is_some_and(|(s, c)| s > c)
    {
        return Err(bad("币种、付款周期或日期范围无效"));
    }
    text(&input.notes, 8000, false)?;
    if !input.url.is_empty() {
        let url = reqwest::Url::parse(&input.url).map_err(|_| bad("网址无效"))?;
        if !["https", "http"].contains(&url.scheme())
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || input.url.len() > 2048
        {
            return Err(bad("网址仅支持不带凭据的 HTTP/HTTPS 地址"));
        }
    }
    Ok(())
}
async fn save(tx: &mut Tx<'_>, id: Option<i64>, input: &Input) -> ApiResult<Subscription> {
    validate(input)?;
    for (label_id, kind) in [
        (input.category_id, "category"),
        (input.payment_method_id, "payment_method"),
        (input.payer_id, "payer"),
    ] {
        if let Some(label_id) = label_id {
            let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM expense_labels l WHERE l.id=$1 AND kind=$2 AND (enabled OR EXISTS(SELECT 1 FROM expense_subscriptions s WHERE s.id=$3 AND CASE $2 WHEN 'category' THEN s.category_id WHEN 'payer' THEN s.payer_id ELSE s.payment_method_id END=l.id)))").bind(label_id).bind(kind).bind(id).fetch_one(&mut **tx).await?;
            if !exists {
                return Err(bad("分类、支付方式或付款人不存在或已停用"));
            }
        }
    }
    if let Some(replacement) = input.replacement_id {
        if Some(replacement) == id {
            return Err(bad("替代订阅不能指向自身"));
        }
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM expense_subscriptions WHERE id=$1)")
                .bind(replacement)
                .fetch_one(&mut **tx)
                .await?;
        if !exists {
            return Err(bad("替代订阅不存在"));
        }
        let cycle:bool=sqlx::query_scalar("WITH RECURSIVE chain AS (SELECT id,replacement_id,ARRAY[id] AS path FROM expense_subscriptions WHERE id=$1 UNION ALL SELECT s.id,s.replacement_id,path||s.id FROM expense_subscriptions s JOIN chain c ON s.id=c.replacement_id WHERE NOT s.id=ANY(path)) SELECT EXISTS(SELECT 1 FROM chain WHERE id=$2)").bind(replacement).bind(id).fetch_one(&mut **tx).await?;
        if cycle {
            return Err(bad("替代订阅关系不能形成循环"));
        }
    }
    let sql = if id.is_some() {
        format!(
            "UPDATE expense_subscriptions SET name=$1,amount_units=$2,currency=$3,start_date=$4::text::date,anchor_payment=CASE WHEN next_payment<>$5::text::date OR cycle<>$6 OR frequency<>$7 THEN $5::text::date ELSE anchor_payment END,next_payment=$5::text::date,cycle=$6,frequency=$7,category_id=$8,payment_method_id=$9,payer_id=$10,notes=$11,url=$12,inactive=$13,auto_renew=$14,notify=$15,notify_days_before=$16,cancellation_date=$17::text::date,replacement_id=$18,updated_at=$19,version=version+1 WHERE id=$20 AND version=$21 RETURNING {COLUMNS}"
        )
    } else {
        format!(
            "INSERT INTO expense_subscriptions(name,amount_units,currency,start_date,next_payment,anchor_payment,cycle,frequency,category_id,payment_method_id,payer_id,notes,url,inactive,auto_renew,notify,notify_days_before,cancellation_date,replacement_id,created_at,updated_at) SELECT $1,$2,$3,$4::text::date,$5::text::date,$5::text::date,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17::text::date,$18,$19,$19 WHERE $20::bigint IS NULL AND $21::bigint IS NULL RETURNING {COLUMNS}"
        )
    };
    let value = sqlx::query_as(&sql)
        .bind(input.name.trim())
        .bind(units(&input.amount_units)?)
        .bind(&input.currency)
        .bind(&input.start_date)
        .bind(&input.next_payment)
        .bind(&input.cycle)
        .bind(input.frequency)
        .bind(input.category_id)
        .bind(input.payment_method_id)
        .bind(input.payer_id)
        .bind(input.notes.trim())
        .bind(&input.url)
        .bind(input.inactive)
        .bind(input.auto_renew)
        .bind(input.notify)
        .bind(input.notify_days_before)
        .bind(&input.cancellation_date)
        .bind(input.replacement_id)
        .bind(now_timestamp())
        .bind(id)
        .bind(input.version)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| conflict("订阅配置已变化，请刷新后重试"))?;
    Ok(value)
}

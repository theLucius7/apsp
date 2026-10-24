use super::*;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};

pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<Subscription>>> {
    actor(&state, &headers, false).await?;
    Ok(Json(sqlx::query_as(&format!("SELECT {COLUMNS} FROM expense_subscriptions ORDER BY inactive,next_payment,id LIMIT 10000")).fetch_all(&state.pool).await?))
}
pub(super) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> ApiResult<Json<Subscription>> {
    let actor = actor(&state, &headers, true).await?;
    let mut tx = state.pool.begin().await?;
    lock(&mut tx).await?;
    let value = save(&mut tx, None, &input).await?;
    event(
        &mut tx,
        Some(value.id),
        "created",
        json!({"administrator_id":actor}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(value))
}
pub(super) async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<Input>,
) -> ApiResult<Json<Subscription>> {
    let actor = actor(&state, &headers, true).await?;
    let mut tx = state.pool.begin().await?;
    lock(&mut tx).await?;
    let value = save(&mut tx, Some(id), &input).await?;
    sqlx::query("UPDATE expense_deliveries SET state='cancelled' WHERE state IN('pending','failed') AND event_id IN(SELECT id FROM expense_events WHERE subscription_id=$1)").bind(id).execute(&mut *tx).await?;
    event(
        &mut tx,
        Some(id),
        "updated",
        json!({"administrator_id":actor,"version":value.version}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(value))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Version {
    version: i64,
}
pub(super) async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<Version>,
) -> ApiResult<Json<Value>> {
    let actor = actor(&state, &headers, true).await?;
    let mut tx = state.pool.begin().await?;
    lock(&mut tx).await?;
    let referenced: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM expense_subscriptions WHERE replacement_id=$1)",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if referenced {
        return Err(conflict("此项目被其他订阅设为替代订阅，请先解除关系"));
    }
    sqlx::query("UPDATE expense_deliveries SET state='cancelled' WHERE state IN('pending','failed') AND event_id IN(SELECT id FROM expense_events WHERE subscription_id=$1)").bind(id).execute(&mut *tx).await?;
    let removed = sqlx::query("DELETE FROM expense_subscriptions WHERE id=$1 AND version=$2")
        .bind(id)
        .bind(input.version)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if removed != 1 {
        return Err(conflict("订阅已变化或已删除，请刷新"));
    }
    event(
        &mut tx,
        None,
        "deleted",
        json!({"administrator_id":actor,"subscription_id":id}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"deleted":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Operation {
    request_id: Uuid,
    version: i64,
}
pub(super) async fn clone_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<Operation>,
) -> ApiResult<Json<Value>> {
    operate(&state, &headers, id, input, false).await
}
pub(super) async fn renew(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<Operation>,
) -> ApiResult<Json<Value>> {
    operate(&state, &headers, id, input, true).await
}
async fn operate(
    state: &AppState,
    headers: &HeaderMap,
    id: i64,
    input: Operation,
    renew: bool,
) -> ApiResult<Json<Value>> {
    let administrator = actor(state, headers, true).await?;
    let mut tx = state.pool.begin().await?;
    lock(&mut tx).await?;
    let key = format!("operation:{}", input.request_id);
    let identity = json!({"id":id,"version":input.version,"renew":renew});
    if let Some(previous) =
        sqlx::query_scalar::<_, Value>("SELECT detail FROM expense_events WHERE event_key=$1")
            .bind(&key)
            .fetch_optional(&mut *tx)
            .await?
    {
        if previous["request"] != identity {
            return Err(conflict("请求编号已用于其他操作"));
        }
        return Ok(Json(previous["result"].clone()));
    }
    let original: Subscription = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM expense_subscriptions WHERE id=$1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    if original.version != input.version {
        return Err(conflict("订阅已变化，请刷新"));
    }
    let result = if renew {
        let today:String=sqlx::query_scalar("SELECT (to_timestamp($1) AT TIME ZONE timezone)::date::text FROM expense_settings WHERE singleton").bind(now_timestamp()).fetch_one(&mut *tx).await?;
        let from = schedule::parse(&original.next_payment)?
            .max(schedule::parse(&today)?)
            .next_day()
            .ok_or_else(|| bad("日期超出范围"))?;
        let (next, _) = schedule::at_or_after(
            schedule::parse(&original.anchor_payment)?,
            &original.cycle,
            original.frequency,
            from,
        )
        .ok_or_else(|| bad("续期日期超出范围"))?;
        if original
            .cancellation_date
            .as_deref()
            .map(schedule::parse)
            .transpose()?
            .is_some_and(|c| next > c)
        {
            return Err(conflict("下次付款超过取消日期，请先调整取消计划"));
        }
        // A confirmed renewal records the payment for the period it covers.
        sqlx::query("INSERT INTO expense_payments(subscription_id,request_id,period_start,paid_on,amount_units,currency,administrator_id,created_at) VALUES($1,$2,$3::text::date,$4::text::date,$5,$6,$7,$8)")
            .bind(id)
            .bind(input.request_id)
            .bind(&original.next_payment)
            .bind(&today)
            .bind(units(&original.amount_units)?)
            .bind(&original.currency)
            .bind(administrator)
            .bind(now_timestamp())
            .execute(&mut *tx)
            .await?;
        let value:Subscription=sqlx::query_as(&format!("UPDATE expense_subscriptions SET next_payment=$2::text::date,inactive=FALSE,version=version+1,updated_at=$3 WHERE id=$1 RETURNING {COLUMNS}")).bind(id).bind(schedule::format(next)).bind(now_timestamp()).fetch_one(&mut *tx).await?;
        serde_json::to_value(value).map_err(anyhow::Error::from)?
    } else {
        let input = Input {
            name: format!("{}（副本）", original.name),
            amount_units: original.amount_units,
            currency: original.currency,
            start_date: original.start_date,
            next_payment: original.next_payment,
            cycle: original.cycle,
            frequency: original.frequency,
            category_id: original.category_id,
            payment_method_id: original.payment_method_id,
            payer_id: original.payer_id,
            notes: original.notes,
            url: original.url,
            inactive: true,
            auto_renew: original.auto_renew,
            notify: false,
            notify_days_before: original.notify_days_before,
            cancellation_date: original.cancellation_date,
            replacement_id: None,
            version: None,
        };
        serde_json::to_value(save(&mut tx, None, &input).await?).map_err(anyhow::Error::from)?
    };
    sqlx::query("INSERT INTO expense_events(id,subscription_id,event_key,kind,detail,created_at) VALUES($1,$2,$3,$4,$5,$6)").bind(Uuid::new_v4()).bind(id).bind(key).bind(if renew{"manual_renewal"}else{"cloned"}).bind(json!({"request":identity,"result":result,"administrator_id":administrator})).bind(now_timestamp()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(result))
}
pub(super) async fn payments(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    actor(&state, &headers, false).await?;
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('id',id,'period_start',period_start::text,'paid_on',paid_on::text,'amount_units',amount_units::text,'currency',currency,'created_at',created_at) FROM expense_payments WHERE subscription_id=$1 ORDER BY paid_on DESC,id DESC LIMIT 200",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({"entries":rows})))
}
pub(super) async fn labels(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    actor(&state, &headers, false).await?;
    let rows: Vec<Value> =
        sqlx::query_scalar("SELECT to_jsonb(l) FROM expense_labels l ORDER BY kind,name,id")
            .fetch_all(&state.pool)
            .await?;
    Ok(Json(json!({"entries":rows})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Label {
    kind: String,
    name: String,
    enabled: bool,
}
pub(super) async fn label_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Label>,
) -> ApiResult<Json<Value>> {
    label_save(&state, &headers, None, input).await
}
pub(super) async fn label_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<Label>,
) -> ApiResult<Json<Value>> {
    label_save(&state, &headers, Some(id), input).await
}
async fn label_save(
    state: &AppState,
    headers: &HeaderMap,
    id: Option<i64>,
    input: Label,
) -> ApiResult<Json<Value>> {
    actor(state, headers, true).await?;
    let name = text(&input.name, 100, true)?;
    if !["category", "payment_method", "payer"].contains(&input.kind.as_str()) {
        return Err(bad("分类类型无效"));
    }
    let mut tx = state.pool.begin().await?;
    lock(&mut tx).await?;
    let duplicate:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM expense_labels WHERE name=$1 AND kind=$2 AND ($3::bigint IS NULL OR id<>$3))").bind(&name).bind(&input.kind).bind(id).fetch_one(&mut *tx).await?;
    if duplicate {
        return Err(conflict("名称已存在"));
    }
    let query = if id.is_some() {
        "UPDATE expense_labels SET name=$1,enabled=$3 WHERE id=$4 AND kind=$2 RETURNING to_jsonb(expense_labels)"
    } else {
        "INSERT INTO expense_labels(name,kind,enabled) SELECT $1,$2,$3 WHERE $4::bigint IS NULL RETURNING to_jsonb(expense_labels)"
    };
    let result = sqlx::query_scalar(query)
        .bind(name)
        .bind(input.kind)
        .bind(input.enabled)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    tx.commit().await?;
    Ok(Json(result))
}
pub(super) async fn label_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    actor(&state, &headers, true).await?;
    let mut tx = state.pool.begin().await?;
    lock(&mut tx).await?;
    let used:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM expense_subscriptions WHERE category_id=$1 OR payment_method_id=$1 OR payer_id=$1)").bind(id).fetch_one(&mut *tx).await?;
    if used {
        return Err(conflict("此项仍被订阅使用，可停用或先修改订阅"));
    }
    sqlx::query("DELETE FROM expense_labels WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"deleted":true})))
}
pub(super) async fn settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    actor(&state, &headers, false).await?;
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(s)||jsonb_build_object('budget_units',budget_units::text) FROM expense_settings s WHERE singleton").fetch_one(&state.pool).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    base_currency: String,
    budget_units: Option<String>,
    timezone: String,
    reminders_enabled: bool,
    version: i64,
}
pub(super) async fn settings_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Settings>,
) -> ApiResult<Json<Value>> {
    let administrator = actor(&state, &headers, true).await?;
    if !currency(&input.base_currency) {
        return Err(bad("本位币须为三位大写币种代码"));
    }
    let budget = input.budget_units.as_deref().map(units).transpose()?;
    let mut tx = state.pool.begin().await?;
    lock(&mut tx).await?;
    let zone: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_timezone_names WHERE name=$1)")
            .bind(&input.timezone)
            .fetch_one(&mut *tx)
            .await?;
    if !zone {
        return Err(bad("请选择有效的 IANA 时区"));
    }
    let result:Option<Value>=sqlx::query_scalar("UPDATE expense_settings SET base_currency=$1,budget_units=$2,timezone=$3,reminders_enabled=$4,version=version+1 WHERE singleton AND version=$5 RETURNING to_jsonb(expense_settings)||jsonb_build_object('budget_units',budget_units::text)").bind(input.base_currency).bind(budget).bind(input.timezone).bind(input.reminders_enabled).bind(input.version).fetch_optional(&mut *tx).await?;
    if !input.reminders_enabled {
        sqlx::query(
            "UPDATE expense_deliveries SET state='cancelled' WHERE state IN('pending','failed')",
        )
        .execute(&mut *tx)
        .await?;
    }
    let result = result.ok_or_else(|| conflict("设置已变化，请刷新"))?;
    event(
        &mut tx,
        None,
        "settings_updated",
        json!({"administrator_id":administrator,"reminders_enabled":input.reminders_enabled}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(result))
}

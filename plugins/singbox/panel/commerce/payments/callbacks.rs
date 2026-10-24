use super::*;
use axum::{Form, extract::Query};

pub(crate) async fn callback_get(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(values): Query<Vec<(String, String)>>,
) -> ApiResult<&'static str> {
    accept(&state, id, values).await
}
pub(crate) async fn callback_post(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(values): Form<Vec<(String, String)>>,
) -> ApiResult<&'static str> {
    accept(&state, id, values).await
}
async fn accept(
    state: &AppState,
    channel_id: i64,
    values: Vec<(String, String)>,
) -> ApiResult<&'static str> {
    if values.len() > 32
        || values.iter().any(|(k, v)| {
            k.len() > 64
                || v.len() > 2048
                || k.chars().any(char::is_control)
                || v.chars().any(char::is_control)
        })
    {
        return Err(bad("支付回调参数过大"));
    }
    let count = values.len();
    let params: BTreeMap<String, String> = values.into_iter().collect();
    if count != params.len() {
        return Err(bad("支付回调存在重复字段"));
    }
    let channel: Channel = sqlx::query_as("SELECT * FROM singbox_payment_channels WHERE id=$1")
        .bind(channel_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(ApiError::NotFound)?;
    let credential = crate::control_center::credentials::resolve_reference(
        state,
        channel.credential_id,
        "external-api",
        "member-payment-callback",
    )
    .await?;
    let signed = params
        .get("sign")
        .ok_or_else(|| bad("支付签名缺失"))?
        .to_ascii_lowercase();
    let expected = signature(&params, key(&credential)?);
    if signed.len() != 32
        || signed
            .as_bytes()
            .iter()
            .zip(expected.as_bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            != 0
        || params.get("sign_type").is_some_and(|s| s != "MD5")
    {
        return Err(bad("支付签名无效"));
    }
    if params.get("trade_status").map(String::as_str) != Some("TRADE_SUCCESS")
        || params.get("pid") != Some(&channel.merchant)
    {
        return Err(bad("支付状态或商户不匹配"));
    }
    if params
        .get("type")
        .is_some_and(|v| v != &channel.payment_type)
    {
        return Err(bad("支付类型不匹配"));
    }
    let id = Uuid::parse_str(
        params
            .get("out_trade_no")
            .ok_or_else(|| bad("业务单号缺失"))?,
    )
    .map_err(|_| bad("业务单号无效"))?;
    let reference = params
        .get("trade_no")
        .filter(|v| !v.is_empty() && v.len() <= 128)
        .ok_or_else(|| bad("支付回执缺失"))?;
    let amount = minor(params.get("money").ok_or_else(|| bad("支付金额缺失"))?)?;
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let topup = sqlx::query("SELECT * FROM singbox_payment_topups WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    if topup.try_get::<i64, _>("channel_id")? != channel_id
        || topup.try_get::<String, _>("merchant")? != channel.merchant
        || topup.try_get::<i64, _>("amount_cents")? + topup.try_get::<i64, _>("fee_cents")?
            != amount
    {
        return Err(bad("支付金额、商户或渠道不匹配"));
    }
    if let Some(prior)=sqlx::query("SELECT topup_id,amount_cents FROM singbox_payment_receipts WHERE channel_id=$1 AND provider_reference=$2").bind(channel_id).bind(reference).fetch_optional(&mut *tx).await? {
        if prior.try_get::<Uuid,_>("topup_id")?!=id || prior.try_get::<i64,_>("amount_cents")?!=amount { return Err(conflict("支付回执已用于另一笔交易")); }
        // Receipt and credit/exception are always committed together.
        return Ok("success");
    }
    let previous: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_payment_receipts WHERE topup_id=$1)",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    let receipt:i64=sqlx::query_scalar("INSERT INTO singbox_payment_receipts(topup_id,channel_id,provider_reference,amount_cents,created_at) VALUES($1,$2,$3,$4,$5) RETURNING id").bind(id).bind(channel_id).bind(reference).bind(amount).bind(now_timestamp()).fetch_one(&mut *tx).await?;
    let user: i64 = topup.try_get("user_id")?;
    let status: String = topup.try_get("state")?;
    let unavailable: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM users WHERE id=$1")
            .bind(user)
            .fetch_one(&mut *tx)
            .await?;
    let code = if previous || status == "paid" {
        Some("duplicate_receipt")
    } else if unavailable {
        Some("member_unavailable")
    } else if status == "cancelled" {
        Some("late_cancelled")
    } else if status == "expired" || topup.try_get::<i64, _>("expires_at")? <= now_timestamp() {
        Some("late_expired")
    } else if wallet::balance(&mut tx, user).await? + topup.try_get::<i64, _>("amount_cents")?
        > MAX_BALANCE
    {
        Some("balance_limit")
    } else {
        None
    };
    if let Some(code) = code {
        sqlx::query("INSERT INTO singbox_payment_exceptions(topup_id,receipt_id,code,created_at) VALUES($1,$2,$3,$4)").bind(id).bind(receipt).bind(code).bind(now_timestamp()).execute(&mut *tx).await?;
        if !previous && status != "paid" {
            sqlx::query("UPDATE singbox_payment_topups SET state='exception' WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        event(
            &mut tx,
            Some(user),
            "epay",
            "payment_exception",
            &id.to_string(),
            json!({"code":code,"receipt_id":receipt}),
        )
        .await?;
    } else {
        credit(&mut tx, id, user, topup.try_get("amount_cents")?).await?;
    }
    tx.commit().await?;
    Ok("success")
}
async fn credit(tx: &mut Tx<'_>, id: Uuid, user: i64, amount: i64) -> ApiResult<()> {
    wallet::post(
        tx,
        wallet::Posting {
            user,
            key: &format!("topup:{id}"),
            action: "recharge",
            delta: amount,
            source: &id.to_string(),
            actor: "epay",
            reason: "已核验支付回执",
        },
    )
    .await?;
    sqlx::query("UPDATE singbox_payment_topups SET state='paid',paid_at=$2 WHERE id=$1")
        .bind(id)
        .bind(now_timestamp())
        .execute(&mut **tx)
        .await?;
    event(
        tx,
        Some(user),
        "epay",
        "topup_credited",
        &id.to_string(),
        json!({"amount_cents":amount}),
    )
    .await?;
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Resolution {
    action: String,
    reason: String,
}
pub(crate) async fn resolve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<Resolution>,
) -> ApiResult<Json<Value>> {
    let actor = administrator(&state, &headers, true).await?;
    let reason = text(&input.reason, 500, true)?;
    if !["credit", "refunded"].contains(&input.action.as_str()) {
        return Err(bad("请选择入账或登记已原路退款"));
    }
    let target = if input.action == "credit" {
        "credited"
    } else {
        "refunded"
    };
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let row = sqlx::query("SELECT * FROM singbox_payment_exceptions WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let current: String = row.try_get("state")?;
    if current != "open" {
        if current != target
            || row.try_get::<Option<String>, _>("resolution")?.as_deref() != Some(reason.as_str())
        {
            return Err(conflict("此异常已处理，请核对历史记录"));
        }
        return Ok(Json(json!({"id":id,"state":current})));
    }
    let topup_id: Uuid = row.try_get("topup_id")?;
    if input.action == "credit" {
        if row.try_get::<String, _>("code")? == "duplicate_receipt" {
            return Err(conflict("重复付款须核对原路退款，不能重复计入账户"));
        }
        let topup = sqlx::query(
            "SELECT user_id,amount_cents,state FROM singbox_payment_topups WHERE id=$1 FOR UPDATE",
        )
        .bind(topup_id)
        .fetch_one(&mut *tx)
        .await?;
        if topup.try_get::<String, _>("state")? == "paid" {
            return Err(conflict("充值已入账"));
        }
        credit(
            &mut tx,
            topup_id,
            topup.try_get("user_id")?,
            topup.try_get("amount_cents")?,
        )
        .await?;
    }
    sqlx::query(
        "UPDATE singbox_payment_exceptions SET state=$2,resolution=$3,resolved_at=$4 WHERE id=$1",
    )
    .bind(id)
    .bind(target)
    .bind(&reason)
    .bind(now_timestamp())
    .execute(&mut *tx)
    .await?;
    event(
        &mut tx,
        None,
        &actor,
        "payment_exception_resolved",
        &id.to_string(),
        json!({"action":input.action,"reason":reason,"refund_executed_by_panel":false}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"state":target})))
}

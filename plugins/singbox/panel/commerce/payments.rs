mod callbacks;
pub(super) use callbacks::{callback_get, callback_post, resolve};

use super::*;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use md5::Md5;
use std::collections::BTreeMap;

#[derive(Clone, Deserialize, Serialize, sqlx::FromRow)]
pub(super) struct Channel {
    id: i64,
    name: String,
    gateway: String,
    merchant: String,
    credential_id: Uuid,
    payment_type: String,
    fixed_fee_cents: i64,
    fee_basis_points: i32,
    enabled: bool,
    version: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChannelInput {
    name: String,
    gateway: String,
    merchant: String,
    credential_id: Uuid,
    payment_type: String,
    fixed_fee_cents: i64,
    fee_basis_points: i32,
    enabled: bool,
    version: Option<i64>,
}
pub(super) async fn channels(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<Channel>>> {
    administrator(&state, &headers, false).await?;
    Ok(Json(
        sqlx::query_as("SELECT * FROM singbox_payment_channels ORDER BY id")
            .fetch_all(&state.pool)
            .await?,
    ))
}
pub(super) async fn channel_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ChannelInput>,
) -> ApiResult<Json<Channel>> {
    let actor = administrator(&state, &headers, true).await?;
    Ok(Json(save(&state, &actor, None, input).await?))
}
pub(super) async fn channel_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<ChannelInput>,
) -> ApiResult<Json<Channel>> {
    let actor = administrator(&state, &headers, true).await?;
    Ok(Json(save(&state, &actor, Some(id), input).await?))
}
async fn save(
    state: &AppState,
    actor: &str,
    id: Option<i64>,
    input: ChannelInput,
) -> ApiResult<Channel> {
    let name = crate::business::name(&input.name)?;
    let merchant = text(&input.merchant, 100, true)?;
    if !merchant
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || !["alipay", "wxpay", "qqpay"].contains(&input.payment_type.as_str())
        || !(0..=1_000_000).contains(&input.fixed_fee_cents)
        || !(0..=10000).contains(&input.fee_basis_points)
    {
        return Err(bad("支付渠道参数无效"));
    }
    let gateway = reqwest::Url::parse(&input.gateway).map_err(|_| bad("支付网关地址无效"))?;
    if gateway.scheme() != "https"
        || gateway.host_str().is_none()
        || !gateway.username().is_empty()
        || gateway.password().is_some()
        || gateway.query().is_some()
        || gateway.fragment().is_some()
    {
        return Err(bad("支付网关必须是无凭据及查询参数的 HTTPS 地址"));
    }
    let credential = crate::control_center::credentials::resolve_reference(
        state,
        input.credential_id,
        "external-api",
        "member-payment-config",
    )
    .await?;
    key(&credential)?;
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    // Immutable merchant/secret bindings preserve verification of old payments.
    if let Some(id) = id {
        let previous: Channel =
            sqlx::query_as("SELECT * FROM singbox_payment_channels WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(ApiError::NotFound)?;
        let used: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM singbox_payment_topups WHERE channel_id=$1)",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if used
            && (previous.merchant != merchant
                || previous.credential_id != input.credential_id
                || previous.gateway != gateway.as_str()
                || previous.payment_type != input.payment_type)
        {
            return Err(conflict(
                "此渠道已有交易；更换商户、密钥或网关请新增渠道，保留旧渠道用于回调核对",
            ));
        }
    }
    let query = if id.is_some() {
        "UPDATE singbox_payment_channels SET name=$1,gateway=$2,merchant=$3,credential_id=$4,payment_type=$5,fixed_fee_cents=$6,fee_basis_points=$7,enabled=$8,updated_at=$9,version=version+1 WHERE id=$10 AND version=$11 RETURNING *"
    } else {
        "INSERT INTO singbox_payment_channels(name,gateway,merchant,credential_id,payment_type,fixed_fee_cents,fee_basis_points,enabled,updated_at) SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9 WHERE $10::bigint IS NULL AND $11::bigint IS NULL RETURNING *"
    };
    let channel: Channel = sqlx::query_as(query)
        .bind(name)
        .bind(gateway.as_str())
        .bind(merchant)
        .bind(input.credential_id)
        .bind(input.payment_type)
        .bind(input.fixed_fee_cents)
        .bind(input.fee_basis_points)
        .bind(input.enabled)
        .bind(now_timestamp())
        .bind(id)
        .bind(input.version)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| conflict("渠道已变化，请刷新"))?;
    event(
        &mut tx,
        None,
        actor,
        "channel_saved",
        &channel.id.to_string(),
        json!({"enabled":channel.enabled,"version":channel.version}),
    )
    .await?;
    tx.commit().await?;
    Ok(channel)
}
fn key(credential: &Value) -> ApiResult<&str> {
    credential["key"]
        .as_str()
        .filter(|s| (16..=256).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_graphic()))
        .ok_or_else(|| bad("支付凭据需要 16–256 字符的 key 字段"))
}
fn fee(amount: i64, channel: &Channel) -> ApiResult<i64> {
    amount
        .checked_mul(i64::from(channel.fee_basis_points))
        .and_then(|v| v.checked_add(5000))
        .map(|v| v / 10000)
        .and_then(|v| v.checked_add(channel.fixed_fee_cents))
        .filter(|v| amount.checked_add(*v).is_some_and(|sum| sum <= MAX_BALANCE))
        .ok_or_else(|| bad("支付手续费超出范围"))
}
fn signature(params: &BTreeMap<String, String>, key: &str) -> String {
    let mut value = params
        .iter()
        .filter(|(k, v)| k.as_str() != "sign" && k.as_str() != "sign_type" && !v.is_empty())
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    value.push_str(key);
    format!("{:x}", Md5::digest(value.as_bytes()))
}
fn decimal(amount: i64) -> String {
    format!("{}.{:02}", amount / 100, amount % 100)
}
fn minor(value: &str) -> ApiResult<i64> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || whole.len() > 8
        || !whole.bytes().all(|c| c.is_ascii_digit())
        || fraction.len() > 2
        || !fraction.bytes().all(|c| c.is_ascii_digit())
        || (value.contains('.') && fraction.is_empty())
    {
        return Err(bad("支付金额格式无效"));
    }
    let major = whole.parse::<i64>().map_err(|_| bad("支付金额无效"))?;
    let fraction = format!("{fraction:0<2}")
        .parse::<i64>()
        .map_err(|_| bad("支付金额无效"))?;
    Ok(major * 100 + fraction)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Topup {
    request_id: Uuid,
    channel_id: i64,
    channel_version: i64,
    amount_cents: i64,
    expected_fee_cents: i64,
}
pub(super) async fn topup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<Uuid>,
    Json(input): Json<Topup>,
) -> ApiResult<Json<Value>> {
    if !(100..=1_000_000).contains(&input.amount_cents) {
        return Err(bad("充值金额须在 1–10000 元之间"));
    }
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let user = member_session(&mut tx, &state, &headers, account, true).await?;
    active(&mut tx, user).await?;
    let hash = fingerprint(&input)?;
    if let Some(row)=sqlx::query("SELECT to_jsonb(t)-'request_hash'-'merchant' AS value,request_hash FROM singbox_payment_topups t WHERE user_id=$1 AND request_id=$2").bind(user).bind(input.request_id).fetch_optional(&mut *tx).await? {
        if row.try_get::<String,_>("request_hash")?!=hash { return Err(conflict("请求编号已用于另一笔充值")); }
        return Ok(Json(row.try_get("value")?));
    }
    compatibility::writable(&mut tx).await?;
    let channel: Channel =
        sqlx::query_as("SELECT * FROM singbox_payment_channels WHERE id=$1 AND enabled FOR UPDATE")
            .bind(input.channel_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    let fee = fee(input.amount_cents, &channel)?;
    if channel.version != input.channel_version || fee != input.expected_fee_cents {
        return Err(conflict("支付渠道或手续费已变化，请重新确认"));
    }
    if wallet::balance(&mut tx, user).await? + input.amount_cents > MAX_BALANCE {
        return Err(conflict("充值后余额超过账户上限"));
    }
    sqlx::query("UPDATE singbox_payment_topups SET state='expired' WHERE user_id=$1 AND state='pending' AND expires_at<=$2").bind(user).bind(now_timestamp()).execute(&mut *tx).await?;
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_payment_topups WHERE user_id=$1 AND state='pending')",
    )
    .bind(user)
    .fetch_one(&mut *tx)
    .await?;
    if pending {
        return Err(conflict("已有待支付充值，请先继续支付或取消"));
    }
    let id = Uuid::new_v4();
    let value:Value=sqlx::query_scalar("INSERT INTO singbox_payment_topups(id,user_id,request_id,request_hash,channel_id,merchant,amount_cents,fee_cents,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING to_jsonb(singbox_payment_topups)-'request_hash'-'merchant'").bind(id).bind(user).bind(input.request_id).bind(hash).bind(channel.id).bind(channel.merchant).bind(input.amount_cents).bind(fee).bind(now_timestamp()).bind(now_timestamp()+1800).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(value))
}
pub(super) async fn checkout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((account, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let user = member_session(&mut tx, &state, &headers, account, true).await?;
    let row=sqlx::query("SELECT * FROM singbox_payment_topups WHERE id=$1 AND user_id=$2 AND state='pending' AND expires_at>$3 FOR UPDATE").bind(id).bind(user).bind(now_timestamp()).fetch_optional(&mut *tx).await?.ok_or_else(||conflict("此充值已结束，请刷新状态"))?;
    let channel: Channel =
        sqlx::query_as("SELECT * FROM singbox_payment_channels WHERE id=$1 AND enabled")
            .bind(row.try_get::<i64, _>("channel_id")?)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| conflict("支付渠道已停用"))?;
    if channel.merchant != row.try_get::<String, _>("merchant")? {
        return Err(conflict("支付商户不匹配"));
    }
    let amount: i64 = row.try_get("amount_cents")?;
    let fee: i64 = row.try_get("fee_cents")?;
    tx.commit().await?;
    let credential = crate::control_center::credentials::resolve_reference(
        &state,
        channel.credential_id,
        "external-api",
        "member-payment-checkout",
    )
    .await?;
    let mut params = BTreeMap::from([
        ("pid".into(), channel.merchant),
        ("type".into(), channel.payment_type),
        ("out_trade_no".into(), id.simple().to_string()),
        (
            "notify_url".into(),
            format!(
                "{}/api/plugins/sing-box/commerce-public/epay/{}",
                state.config.public_url, channel.id
            ),
        ),
        (
            "return_url".into(),
            format!(
                "{}/#/plugins/sing-box/account/{account}",
                state.config.public_url
            ),
        ),
        ("name".into(), "账户充值".into()),
        ("money".into(), decimal(amount + fee)),
    ]);
    params.insert("sign".into(), signature(&params, key(&credential)?));
    params.insert("sign_type".into(), "MD5".into());
    Ok(Json(
        json!({"action":channel.gateway,"fields":params,"amount_cents":amount,"fee_cents":fee}),
    ))
}
pub(super) async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((account, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let user = member_session(&mut tx, &state, &headers, account, true).await?;
    let status: String = sqlx::query_scalar(
        "SELECT state FROM singbox_payment_topups WHERE id=$1 AND user_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(user)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    if status != "pending" && status != "cancelled" {
        return Err(conflict("此充值已结束，不能取消"));
    }
    sqlx::query("UPDATE singbox_payment_topups SET state='cancelled' WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"state":"cancelled"})))
}
pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    administrator(&state, &headers, false).await?;
    let entries:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(t)-'request_hash'-'merchant' FROM singbox_payment_topups t ORDER BY created_at DESC,id LIMIT 100 OFFSET $1").bind(page.offset()).fetch_all(&state.pool).await?;
    let exceptions:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(e)||jsonb_build_object('amount_cents',r.amount_cents,'provider_reference',r.provider_reference,'user_id',t.user_id) FROM singbox_payment_exceptions e JOIN singbox_payment_receipts r ON r.id=e.receipt_id JOIN singbox_payment_topups t ON t.id=e.topup_id WHERE e.state='open' ORDER BY e.id LIMIT 100").fetch_all(&state.pool).await?;
    Ok(Json(
        json!({"entries":entries,"exceptions":exceptions,"offset":page.offset()}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn amounts_are_exact_and_reject_exponents_or_rounded_inputs() {
        for value in [
            "-1",
            "+1",
            "1e2",
            "1.001",
            "1.",
            ".01",
            "NaN",
            "999999999",
            " 1.00",
        ] {
            assert!(minor(value).is_err(), "{value}");
        }
        assert_eq!(minor("123.4").unwrap(), 12340);
        assert_eq!(minor("0.01").unwrap(), 1);
        assert_eq!(decimal(101), "1.01");
    }
    #[test]
    fn signature_sorts_fields_and_ignores_sign_and_empty_values() {
        let mut input = BTreeMap::from([
            ("b".into(), "2".into()),
            ("a".into(), "1".into()),
            ("empty".into(), "".into()),
        ]);
        assert_eq!(
            signature(&input, "test"),
            format!("{:x}", Md5::digest(b"a=1&b=2test"))
        );
        let expected = signature(&input, "test");
        input.insert("sign".into(), "untrusted".into());
        input.insert("sign_type".into(), "MD5".into());
        assert_eq!(signature(&input, "test"), expected);
    }
}

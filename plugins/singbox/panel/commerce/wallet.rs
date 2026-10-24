use super::*;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};

pub(super) struct Posting<'a> {
    pub user: i64,
    pub key: &'a str,
    pub action: &'a str,
    pub delta: i64,
    pub source: &'a str,
    pub actor: &'a str,
    pub reason: &'a str,
}

pub(super) async fn balance(tx: &mut Tx<'_>, user: i64) -> ApiResult<i64> {
    ensure_member(tx, user).await?;
    Ok(
        sqlx::query_scalar("SELECT balance_cents FROM singbox_wallets WHERE user_id=$1 FOR UPDATE")
            .bind(user)
            .fetch_one(&mut **tx)
            .await?,
    )
}

pub(super) async fn post(tx: &mut Tx<'_>, input: Posting<'_>) -> ApiResult<i64> {
    let Posting {
        user,
        key,
        action,
        delta,
        source,
        actor,
        reason,
    } = input;
    let hash = fingerprint(&(user, action, delta, source, actor, reason))?;
    let before = balance(tx, user).await?;
    if let Some(row) = sqlx::query(
        "SELECT id,request_hash FROM singbox_wallet_ledger WHERE user_id=$1 AND request_key=$2",
    )
    .bind(user)
    .bind(key)
    .fetch_optional(&mut **tx)
    .await?
    {
        if row.try_get::<String, _>("request_hash")? != hash {
            return Err(conflict("请求编号已用于其他资金操作"));
        }
        return Ok(row.try_get("id")?);
    }
    let after = before
        .checked_add(delta)
        .filter(|v| (0..=MAX_BALANCE).contains(v))
        .ok_or_else(|| conflict("余额不足或超过账户余额上限"))?;
    let id=sqlx::query_scalar("INSERT INTO singbox_wallet_ledger(user_id,request_key,request_hash,action,delta_cents,balance_before,balance_after,source_id,actor,reason,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING id")
        .bind(user).bind(key).bind(hash).bind(action).bind(delta).bind(before).bind(after).bind(source).bind(actor).bind(reason).bind(now_timestamp()).fetch_one(&mut **tx).await?;
    sqlx::query("UPDATE singbox_wallets SET balance_cents=$2 WHERE user_id=$1")
        .bind(user)
        .bind(after)
        .execute(&mut **tx)
        .await?;
    Ok(id)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Adjustment {
    request_id: Uuid,
    delta_cents: i64,
    expected_balance_cents: i64,
    reason: String,
}

pub(super) async fn adjust(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<Adjustment>,
) -> ApiResult<Json<Value>> {
    let actor = administrator(&state, &headers, true).await?;
    let reason = text(&input.reason, 500, true)?;
    if input.delta_cents == 0 || !(-MAX_BALANCE..=MAX_BALANCE).contains(&input.delta_cents) {
        return Err(bad("调整金额必须为非零的整数分"));
    }
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let balance = balance(&mut tx, id).await?;
    let key = format!("adjust:{}", input.request_id);
    let prior: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM singbox_wallet_ledger WHERE user_id=$1 AND request_key=$2",
    )
    .bind(id)
    .bind(&key)
    .fetch_optional(&mut *tx)
    .await?;
    if prior.is_none() && balance != input.expected_balance_cents {
        return Err(conflict("余额已变化，请刷新后重新确认"));
    }
    let entry = post(
        &mut tx,
        Posting {
            user: id,
            key: &key,
            action: "admin_adjustment",
            delta: input.delta_cents,
            source: &input.request_id.to_string(),
            actor: &actor,
            reason: &reason,
        },
    )
    .await?;
    if prior.is_none() {
        event(
            &mut tx,
            Some(id),
            &actor,
            "wallet_adjustment",
            &entry.to_string(),
            json!({"delta_cents":input.delta_cents,"reason":reason}),
        )
        .await?;
    }
    let result: Value =
        sqlx::query_scalar("SELECT to_jsonb(l) FROM singbox_wallet_ledger l WHERE id=$1")
            .bind(entry)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(Json(result))
}

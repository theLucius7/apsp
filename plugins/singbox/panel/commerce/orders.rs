use super::*;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Purchase {
    request_id: Uuid,
    offer_id: i64,
    offer_version: i64,
    beneficiary_id: Option<i64>,
}

pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    administrator(&state, &headers, false).await?;
    let entries:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(o)-'request_hash' FROM singbox_commerce_orders o ORDER BY created_at DESC,id DESC LIMIT 100 OFFSET $1").bind(page.offset()).fetch_all(&state.pool).await?;
    Ok(Json(json!({"entries":entries,"offset":page.offset()})))
}
pub(super) async fn purchase(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<Uuid>,
    Json(input): Json<Purchase>,
) -> ApiResult<Json<Value>> {
    state.passkeys.check_origin(&headers)?;
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let payer = member_session(&mut tx, &state, &headers, account, true).await?;
    active(&mut tx, payer).await?;
    let beneficiary = input.beneficiary_id.unwrap_or(payer);
    let hash = fingerprint(&(input.offer_id, input.offer_version, beneficiary))?;
    if let Some(row)=sqlx::query("SELECT to_jsonb(o)-'request_hash' AS value,request_hash FROM singbox_commerce_orders o WHERE payer_id=$1 AND request_id=$2").bind(payer).bind(input.request_id).fetch_optional(&mut *tx).await? {
        if row.try_get::<String,_>("request_hash")?!=hash { return Err(conflict("请求编号已用于另一笔订单")); }
        return Ok(Json(row.try_get("value")?));
    }
    ensure_member(&mut tx, beneficiary).await?;
    if beneficiary != payer {
        let allowed:bool=sqlx::query_scalar("SELECT inviter_id=$2 AND sponsor_required AND first_purchase_at IS NULL AND state='pending' FROM singbox_members WHERE user_id=$1").bind(beneficiary).bind(payer).fetch_one(&mut *tx).await?;
        if !allowed {
            return Err(ApiError::Forbidden(
                "只能为自己邀请且尚未开通的成员购买首单".into(),
            ));
        }
    }
    let offer = catalog::snapshot(
        &mut tx,
        input.offer_id,
        input.offer_version,
        Some(beneficiary),
    )
    .await?;
    let id = Uuid::new_v4();
    wallet::post(
        &mut tx,
        wallet::Posting {
            user: payer,
            key: &format!("order:{id}"),
            action: if payer == beneficiary {
                "plan_purchase"
            } else {
                "sponsor_purchase"
            },
            delta: -offer.price_cents,
            source: &id.to_string(),
            actor: &format!("member:{payer}"),
            reason: "购买套餐",
        },
    )
    .await?;
    let assignment = catalog::deliver(&mut tx, beneficiary, id, &offer).await?;
    let snapshot = serde_json::to_value(&offer).map_err(anyhow::Error::from)?;
    let value:Value=sqlx::query_scalar("INSERT INTO singbox_commerce_orders(id,payer_id,beneficiary_id,request_id,request_hash,offer_id,snapshot,amount_cents,assignment_id,state,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'fulfilled',$10) RETURNING to_jsonb(singbox_commerce_orders)-'request_hash'")
        .bind(id).bind(payer).bind(beneficiary).bind(input.request_id).bind(hash).bind(offer.id).bind(snapshot).bind(offer.price_cents).bind(assignment).bind(now_timestamp()).fetch_one(&mut *tx).await?;
    event(
        &mut tx,
        Some(beneficiary),
        &format!("member:{payer}"),
        "order_fulfilled",
        &id.to_string(),
        json!({"amount_cents":offer.price_cents,"assignment_id":assignment}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(value))
}

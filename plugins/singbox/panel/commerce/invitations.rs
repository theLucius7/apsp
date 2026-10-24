use super::*;
use crate::auth::{hash_token, random_token};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};

fn one() -> i32 {
    1
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Create {
    #[serde(default = "one")]
    quantity: i32,
    request_id: Uuid,
    offer_id: Option<i64>,
    offer_version: Option<i64>,
    expected_admission_cents: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Token {
    token: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Claim {
    token: String,
    request_id: Uuid,
    name: String,
    email: String,
    #[serde(default)]
    challenge_id: Option<Uuid>,
    #[serde(default)]
    code: Option<String>,
}

const VIEW: &str = "to_jsonb(i)-'token_hash'-'request_hash'-'claim_hash'";

pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    administrator(&state, &headers, false).await?;
    let entries:Vec<Value>=sqlx::query_scalar(&format!("SELECT {VIEW} FROM singbox_commerce_invitations i ORDER BY created_at DESC,id LIMIT 100 OFFSET $1")).bind(page.offset()).fetch_all(&state.pool).await?;
    Ok(Json(json!({"entries":entries,"offset":page.offset()})))
}
fn link(state: &AppState, token: &str) -> String {
    format!(
        "{}/#/plugins/sing-box/join/{token}",
        state.config.public_url
    )
}

pub(super) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<Uuid>,
    Json(input): Json<Create>,
) -> ApiResult<Json<Value>> {
    if !(1..=20).contains(&input.quantity) {
        return Err(bad("单次可购买 1 至 20 份邀请"));
    }
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let user = member_session(&mut tx, &state, &headers, account, true).await?;
    active(&mut tx, user).await?;
    let hash = fingerprint(&input)?;
    if let Some(row)=sqlx::query(&format!("SELECT {VIEW} AS value,request_hash FROM singbox_commerce_invitations i WHERE issuer_id=$1 AND request_id=$2")).bind(user).bind(input.request_id).fetch_optional(&mut *tx).await? {
        if row.try_get::<String,_>("request_hash")?!=hash{return Err(conflict("请求编号已用于其他邀请购买"));}
        let entries:Vec<Value>=sqlx::query_scalar(&format!("SELECT {VIEW} FROM singbox_commerce_invitations i WHERE issuer_id=$1 AND batch_id=$2 ORDER BY created_at,id")).bind(user).bind(input.request_id).fetch_all(&mut *tx).await?;
        return Ok(Json(json!({"invitation":row.try_get::<Value,_>("value")?,"invitations":entries,"url":null,"urls":[],"message":"邀请已购买；未保存的链接可在列表中重新生成。"})));
    }
    compatibility::writable(&mut tx).await?;
    let (allowed,admission):(bool,i64)=sqlx::query_as("SELECT invitations_enabled,invite_price_cents FROM singbox_commerce_settings WHERE singleton FOR UPDATE").fetch_one(&mut *tx).await?;
    if !allowed {
        return Err(ApiError::Forbidden("暂未开放购买邀请".into()));
    }
    if admission != input.expected_admission_cents {
        return Err(conflict("邀请价格已变化，请刷新后确认"));
    }
    let mut invitations = Vec::new();
    let mut urls = Vec::new();
    for index in 0..input.quantity {
        let offer = match (input.offer_id, input.offer_version) {
            (Some(id), Some(version)) => Some(catalog::snapshot(&mut tx, id, version, None).await?),
            (None, None) => None,
            _ => return Err(bad("预付套餐和版本必须一起提供")),
        };
        let prepaid = offer.as_ref().map(|o| o.price_cents).unwrap_or(0);
        if (admission + prepaid) * i64::from(input.quantity) > 100_000_000 {
            return Err(bad("单次购买金额超过上限"));
        }
        let id = Uuid::new_v4();
        let token = random_token();
        let ledger = wallet::post(
            &mut tx,
            wallet::Posting {
                user,
                key: &format!("invitation:{id}"),
                action: if offer.is_some() {
                    "invite_prepaid_purchase"
                } else {
                    "invite_purchase"
                },
                delta: -(admission + prepaid),
                source: &id.to_string(),
                actor: &format!("member:{user}"),
                reason: "购买邀请",
            },
        )
        .await?;
        let snapshot = offer
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(anyhow::Error::from)?;
        sqlx::query("INSERT INTO singbox_commerce_invitations(id,issuer_id,request_id,request_hash,token_hash,offer_id,snapshot,admission_cents,prepaid_cents,funding_ledger_id,created_at,batch_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)").bind(id).bind(user).bind(if index==0{input.request_id}else{Uuid::new_v4()}).bind(&hash).bind(hash_token(&token)).bind(input.offer_id).bind(snapshot).bind(admission).bind(prepaid).bind(ledger).bind(now_timestamp()).bind(input.request_id).execute(&mut *tx).await?;
        event(&mut tx,Some(user),&format!("member:{user}"),"invitation_purchased",&id.to_string(),json!({"admission_cents":admission,"prepaid_cents":prepaid,"batch_id":input.request_id})).await?;
        let invitation: Value = sqlx::query_scalar(&format!(
            "SELECT {VIEW} FROM singbox_commerce_invitations i WHERE id=$1"
        ))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        invitations.push(invitation);
        urls.push(link(&state, &token));
    }
    tx.commit().await?;
    Ok(Json(
        json!({"invitation":invitations[0],"invitations":invitations,"url":urls[0],"urls":urls}),
    ))
}

pub(super) async fn rotate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((account, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let user = member_session(&mut tx, &state, &headers, account, true).await?;
    active(&mut tx, user).await?;
    let token = random_token();
    let changed=sqlx::query("UPDATE singbox_commerce_invitations SET token_hash=$3 WHERE id=$1 AND issuer_id=$2 AND state='available'").bind(id).bind(user).bind(hash_token(&token)).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(conflict("邀请已使用或已撤销，不能重新生成链接"));
    }
    event(
        &mut tx,
        Some(user),
        &format!("member:{user}"),
        "invitation_rotated",
        &id.to_string(),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"url":link(&state,&token)})))
}

pub(super) async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((account, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let user = member_session(&mut tx, &state, &headers, account, true).await?;
    let row = sqlx::query(
        "SELECT * FROM singbox_commerce_invitations WHERE id=$1 AND issuer_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(user)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let status: String = row.try_get("state")?;
    if status == "claimed" {
        return Err(conflict("邀请已被领取，不能退款或撤销已交付套餐"));
    }
    if status == "available" {
        let prepaid: i64 = if row.try_get::<Option<i64>, _>("offer_id")?.is_some() {
            row.try_get::<i64, _>("prepaid_cents")? + row.try_get::<i64, _>("admission_cents")?
        } else {
            0
        };
        if prepaid > 0 {
            wallet::post(
                &mut tx,
                wallet::Posting {
                    user,
                    key: &format!("invitation-refund:{id}"),
                    action: "invite_refund",
                    delta: prepaid,
                    source: &id.to_string(),
                    actor: &format!("member:{user}"),
                    reason: "未领取预付邀请全额退款（含准入费）",
                },
            )
            .await?;
        }
        sqlx::query(
            "UPDATE singbox_commerce_invitations SET state='revoked',revoked_at=$2 WHERE id=$1",
        )
        .bind(id)
        .bind(now_timestamp())
        .execute(&mut *tx)
        .await?;
        event(
            &mut tx,
            Some(user),
            &format!("member:{user}"),
            "invitation_revoked",
            &id.to_string(),
            json!({"refunded_cents":prepaid}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"id":id,"state":"revoked"})))
}

fn valid_token(token: &str) -> ApiResult<()> {
    if token.len() != 43
        || !token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(ApiError::NotFound);
    }
    Ok(())
}
pub(super) async fn inspect(
    State(state): State<AppState>,
    Json(input): Json<Token>,
) -> ApiResult<Json<Value>> {
    valid_token(&input.token)?;
    let mut tx = state.pool.begin().await?;
    enabled(&mut tx).await?;
    let row=sqlx::query("SELECT i.state,i.snapshot,m.state AS issuer_state FROM singbox_commerce_invitations i JOIN singbox_members m ON m.user_id=i.issuer_id JOIN users u ON u.id=m.user_id WHERE token_hash=$1 AND u.deleted_at IS NULL").bind(hash_token(&input.token)).fetch_optional(&mut *tx).await?.ok_or(ApiError::NotFound)?;
    let snapshot: Option<Value> = row.try_get("snapshot")?;
    Ok(Json(
        json!({"available":row.try_get::<String,_>("state")?=="available" && row.try_get::<String,_>("issuer_state")?=="active","prepaid":snapshot.is_some(),"package_name":snapshot.as_ref().and_then(|s|s["name"].as_str()),"configuration":state.passkeys.info(),"email_verification_required":mail::config(&state).await?.enabled}),
    ))
}

pub(super) async fn claim(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Claim>,
) -> ApiResult<Json<Value>> {
    state.passkeys.check_origin(&headers)?;
    valid_token(&input.token)?;
    let name = crate::business::name(&input.name)?;
    let email = email(&input.email)?;
    let hash = fingerprint(&(input.request_id, &name, &email))?;
    let verification_required = mail::config(&state).await?.enabled;
    let claimed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_commerce_invitations WHERE token_hash=$1 AND claim_hash=$2 AND state='claimed')").bind(hash_token(&input.token)).bind(&hash).fetch_one(&state.pool).await?;
    let challenge = if !claimed && let Some(id) = input.challenge_id {
        Some(
            accounts::check(
                &state,
                id,
                "invite",
                input.code.as_deref().unwrap_or_default(),
                None,
            )
            .await?,
        )
    } else {
        None
    };
    if let Some(challenge) = &challenge
        && (challenge.address != email
            || challenge.invitation_hash.as_deref() != Some(&hash_token(&input.token)))
    {
        return Err(bad("邮箱验证码与本邀请不匹配"));
    }
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    enabled(&mut tx).await?;
    let row =
        sqlx::query("SELECT * FROM singbox_commerce_invitations WHERE token_hash=$1 FOR UPDATE")
            .bind(hash_token(&input.token))
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    let id: Uuid = row.try_get("id")?;
    let issuer: i64 = row.try_get("issuer_id")?;
    ensure_member(&mut tx, issuer).await?;
    active(&mut tx, issuer).await?;
    let status: String = row.try_get("state")?;
    let (user, account) = if status == "claimed" {
        if row.try_get::<Option<String>, _>("claim_hash")?.as_deref() != Some(&hash) {
            return Err(conflict("邀请已被领取"));
        }
        let user: i64 = row.try_get("recipient_id")?;
        let account: Uuid =
            sqlx::query_scalar("SELECT account_id FROM singbox_portal_accounts WHERE user_id=$1")
                .bind(user)
                .fetch_one(&mut *tx)
                .await?;
        (user, account)
    } else {
        if status != "available" {
            return Err(conflict("邀请已撤销"));
        }
        if verification_required && challenge.is_none() {
            return Err(bad("请先验证联系邮箱"));
        }
        if let Some(challenge) = &challenge {
            accounts::consume(&mut tx, challenge).await?;
        }
        let funded:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_wallet_ledger WHERE id=$1 AND user_id=$2 AND source_id=$3 AND delta_cents=$4 AND action=ANY($5))").bind(row.try_get::<i64,_>("funding_ledger_id")?).bind(issuer).bind(id.to_string()).bind(-(row.try_get::<i64,_>("admission_cents")?+row.try_get::<i64,_>("prepaid_cents")?)).bind(vec!["invite_purchase","invite_prepaid_purchase"]).fetch_one(&mut *tx).await?;
        if !funded {
            return Err(conflict("邀请缺少有效付款记录，请联系管理员"));
        }
        let duplicate: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_members WHERE email=$1)")
                .bind(&email)
                .fetch_one(&mut *tx)
                .await?;
        if duplicate {
            return Err(conflict("此联系邮箱已登记，请使用原账户入口或联系管理员"));
        }
        let user: i64 = sqlx::query_scalar(
            "INSERT INTO users(name,subscription_token) VALUES($1,$2) RETURNING id",
        )
        .bind(name)
        .bind(random_token())
        .fetch_one(&mut *tx)
        .await?;
        let snapshot: Option<Value> = row.try_get("snapshot")?;
        sqlx::query("INSERT INTO singbox_members(user_id,email,state,inviter_id,sponsor_required,created_at) VALUES($1,$2,'pending',$3,$4,$5)").bind(user).bind(email).bind(issuer).bind(snapshot.is_none()).bind(now_timestamp()).execute(&mut *tx).await?;
        if challenge.is_some() {
            sqlx::query("UPDATE singbox_members SET email_verified_at=$2 WHERE user_id=$1")
                .bind(user)
                .bind(now_timestamp())
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("INSERT INTO singbox_wallets(user_id) VALUES($1)")
            .bind(user)
            .execute(&mut *tx)
            .await?;
        let account = Uuid::new_v4();
        sqlx::query("INSERT INTO passkey_accounts(id) VALUES($1)")
            .bind(account)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO singbox_portal_accounts(user_id,account_id) VALUES($1,$2)")
            .bind(user)
            .bind(account)
            .execute(&mut *tx)
            .await?;
        if let Some(snapshot) = snapshot {
            let offer: catalog::Offer =
                serde_json::from_value(snapshot.clone()).map_err(anyhow::Error::from)?;
            let assignment = catalog::deliver(&mut tx, user, id, &offer).await?;
            sqlx::query("INSERT INTO singbox_commerce_orders(id,payer_id,beneficiary_id,request_id,request_hash,offer_id,snapshot,amount_cents,assignment_id,state,created_at) VALUES($1,$2,$3,$1,$4,$5,$6,$7,$8,'fulfilled',$9)").bind(id).bind(issuer).bind(user).bind(&hash).bind(offer.id).bind(snapshot).bind(offer.price_cents).bind(assignment).bind(now_timestamp()).execute(&mut *tx).await?;
        }
        if row.try_get::<Option<Value>, _>("snapshot")?.is_none() {
            messages::activation(&mut tx, user, issuer).await?;
        }
        sqlx::query("UPDATE singbox_commerce_invitations SET state='claimed',recipient_id=$2,claimed_at=$3,claim_hash=$4 WHERE id=$1").bind(id).bind(user).bind(now_timestamp()).bind(&hash).execute(&mut *tx).await?;
        event(
            &mut tx,
            Some(user),
            "invitation",
            "invitation_claimed",
            &id.to_string(),
            json!({"issuer_id":issuer}),
        )
        .await?;
        (user, account)
    };
    let has_keys:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM passkey_credentials WHERE account_id=$1) OR EXISTS(SELECT 1 FROM singbox_members m JOIN singbox_portal_accounts p ON p.user_id=m.user_id WHERE p.account_id=$1 AND m.password_hash IS NOT NULL)").bind(account).fetch_one(&mut *tx).await?;
    let mut url = format!(
        "{}/#/plugins/sing-box/account/{account}",
        state.config.public_url
    );
    if !has_keys {
        let token = random_token();
        sqlx::query("UPDATE singbox_portal_accounts SET activation_hash=$2,activation_expires_at=$3 WHERE user_id=$1").bind(user).bind(hash_token(&token)).bind(now_timestamp()+900).execute(&mut *tx).await?;
        url.push_str(&format!("?activate={token}"));
    }
    tx.commit().await?;
    Ok(Json(json!({"url":url})))
}

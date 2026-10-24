use super::*;
use rand::Rng;

#[derive(sqlx::FromRow)]
pub(crate) struct Challenge {
    pub id: Uuid,
    pub user_id: Option<i64>,
    pub address: String,
    pub old_address: Option<String>,
    pub invitation_hash: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    email: String,
    purpose: String,
    invitation: Option<String>,
}
pub(crate) async fn challenge(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<Request>,
) -> ApiResult<Json<Value>> {
    state.passkeys.check_origin(&headers)?;
    let address = email(&input.email)?;
    if !["invite", "reset"].contains(&input.purpose.as_str()) {
        return Err(bad("验证码用途无效"));
    }
    limit(&state, "mail-global", "all", 60, 60).await?;
    limit(&state, "mail-ip", &peer.ip().to_string(), 5, 60).await?;
    limit(&state, "mail-address", &address, 1, 60).await?;
    let mut tx = state.pool.begin().await?;
    enabled(&mut tx).await?;
    let (user, invitation) = if input.purpose == "invite" {
        let token = input
            .invitation
            .as_deref()
            .filter(|s| s.len() == 43)
            .ok_or(ApiError::NotFound)?;
        let hash = crate::auth::hash_token(token);
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_commerce_invitations i JOIN singbox_members m ON m.user_id=i.issuer_id JOIN users u ON u.id=m.user_id WHERE i.token_hash=$1 AND i.state='available' AND m.state='active' AND u.deleted_at IS NULL)").bind(&hash).fetch_one(&mut *tx).await?;
        if !valid {
            return Err(ApiError::NotFound);
        }
        (None, Some(hash))
    } else {
        let user:Option<i64>=sqlx::query_scalar("SELECT m.user_id FROM singbox_members m JOIN users u ON u.id=m.user_id WHERE m.email=$1 AND m.state IN('active','pending') AND u.deleted_at IS NULL").bind(&address).fetch_optional(&mut *tx).await?;
        (user, None)
    };
    tx.commit().await?;
    let id = if input.purpose == "reset" && user.is_none() {
        Uuid::new_v4()
    } else {
        match send(
            &state,
            &input.purpose,
            user,
            &address,
            None,
            invitation.as_deref(),
        )
        .await
        {
            Ok(id) => id,
            Err(_) if input.purpose == "reset" => Uuid::new_v4(),
            Err(error) => return Err(error),
        }
    };
    Ok(Json(
        json!({"challenge_id":id,"message":"若此地址可用，验证码将发送到邮箱；请在十分钟内完成验证。"}),
    ))
}
pub(super) async fn send(
    state: &AppState,
    purpose: &str,
    user: Option<i64>,
    address: &str,
    old: Option<&str>,
    invitation: Option<&str>,
) -> ApiResult<Uuid> {
    let config = super::super::mail::config(state).await?;
    if !super::super::mail::allowed(&config, address)
        || old.is_some_and(|v| !super::super::mail::allowed(&config, v))
    {
        return Err(conflict(
            "当前邮件设置无法投递此地址，请联系管理员或使用 Passkey",
        ));
    }
    let id = Uuid::new_v4();
    let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000));
    let old_code = old.map(|_| format!("{:06}", rand::thread_rng().gen_range(0..1_000_000)));
    let now = now_timestamp();
    sqlx::query("INSERT INTO singbox_account_challenges(id,purpose,user_id,address,old_address,token_hash,old_token_hash,invitation_hash,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)").bind(id).bind(purpose).bind(user).bind(address).bind(old).bind(crate::auth::hash_token(&format!("{id}:{code}"))).bind(old_code.as_ref().map(|c|crate::auth::hash_token(&format!("{id}:{c}")))).bind(invitation).bind(now).bind(now+600).execute(&state.pool).await?;
    let result=async {
        super::super::mail::send(state,&config,address,"apsp 成员邮箱验证码",&format!("验证码：{code}\n有效期十分钟。若不是你发起的操作，请忽略此邮件。请勿将验证码转交他人。\n挑战编号：{id}")).await?;
        if let (Some(old),Some(code))=(old,old_code){super::super::mail::send(state,&config,old,"apsp 成员邮箱变更验证",&format!("你正在修改成员联系邮箱。旧邮箱验证码：{code}\n有效期十分钟。若非本人操作，请立即检查账户安全。\n挑战编号：{id}")).await?;}
        Ok::<_,ApiError>(())
    }.await;
    if result.is_err() {
        sqlx::query("UPDATE singbox_account_challenges SET consumed_at=$2 WHERE id=$1")
            .bind(id)
            .bind(now_timestamp())
            .execute(&state.pool)
            .await?;
    }
    result?;
    Ok(id)
}
pub(crate) async fn check(
    state: &AppState,
    id: Uuid,
    purpose: &str,
    code: &str,
    old_code: Option<&str>,
) -> ApiResult<Challenge> {
    let mut tx = state.pool.begin().await?;
    let row=sqlx::query("SELECT * FROM singbox_account_challenges WHERE id=$1 AND purpose=$2 AND expires_at>$3 AND consumed_at IS NULL AND attempts<5 FOR UPDATE").bind(id).bind(purpose).bind(now_timestamp()).fetch_optional(&mut *tx).await?.ok_or_else(||conflict("验证码无效、过期或已使用"))?;
    sqlx::query("UPDATE singbox_account_challenges SET attempts=attempts+1 WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let old: Option<String> = row.try_get("old_token_hash")?;
    let valid = code.len() == 6
        && code.bytes().all(|b| b.is_ascii_digit())
        && row.try_get::<String, _>("token_hash")?
            == crate::auth::hash_token(&format!("{id}:{code}"))
        && old.is_none_or(|hash| {
            old_code.is_some_and(|value| {
                value.len() == 6 && hash == crate::auth::hash_token(&format!("{id}:{value}"))
            })
        });
    let result = Challenge {
        id,
        user_id: row.try_get("user_id")?,
        address: row.try_get("address")?,
        old_address: row.try_get("old_address")?,
        invitation_hash: row.try_get("invitation_hash")?,
    };
    tx.commit().await?;
    if !valid {
        return Err(conflict("验证码无效、过期或已使用"));
    }
    Ok(result)
}
pub(crate) async fn consume(tx: &mut Tx<'_>, challenge: &Challenge) -> ApiResult<()> {
    let changed=sqlx::query("UPDATE singbox_account_challenges SET consumed_at=$2 WHERE id=$1 AND consumed_at IS NULL AND expires_at>$2").bind(challenge.id).bind(now_timestamp()).execute(&mut **tx).await?.rows_affected();
    if changed != 1 {
        return Err(conflict("验证码已使用或已过期，请重新获取"));
    }
    Ok(())
}

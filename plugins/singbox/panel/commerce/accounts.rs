mod challenges;
use super::*;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    response::Response,
};
pub(super) use challenges::{challenge, check, consume};
use rand::rngs::OsRng;
use std::net::SocketAddr;

pub(super) async fn limit(
    state: &AppState,
    scope: &str,
    value: &str,
    maximum: i32,
    seconds: i64,
) -> ApiResult<()> {
    let bucket = crate::auth::hash_token(&format!("{scope}:{value}"));
    let now = now_timestamp();
    let attempts:i32=sqlx::query_scalar("INSERT INTO singbox_account_limits(bucket,starts_at,attempts) VALUES($1,$2,1) ON CONFLICT(bucket) DO UPDATE SET attempts=CASE WHEN singbox_account_limits.starts_at<=$2-$3 THEN 1 ELSE singbox_account_limits.attempts+1 END,starts_at=CASE WHEN singbox_account_limits.starts_at<=$2-$3 THEN $2 ELSE singbox_account_limits.starts_at END RETURNING attempts").bind(bucket).bind(now).bind(seconds).fetch_one(&state.pool).await?;
    if attempts > maximum {
        return Err(ApiError::Busy);
    }
    Ok(())
}
async fn password(state: &AppState, value: String) -> ApiResult<String> {
    if value.len() < 12 || value.len() > 1024 {
        return Err(bad("密码长度须为 12 至 1024 字节"));
    }
    let permit = state
        .login_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::Busy)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        Argon2::default()
            .hash_password(value.as_bytes(), &SaltString::generate(&mut OsRng))
            .map(|v| v.to_string())
            .map_err(|_| bad("密码处理失败"))
    })
    .await
    .map_err(anyhow::Error::from)?
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Login {
    email: String,
    password: String,
}
pub(super) async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> ApiResult<Response> {
    state.passkeys.check_origin(&headers)?;
    let address = email(&input.email)?;
    limit(&state, "login-ip", &peer.ip().to_string(), 30, 60).await?;
    limit(&state, "login-email", &address, 10, 600).await?;
    if input.password.len() > 1024 {
        return Err(ApiError::Unauthorized);
    }
    let row:Option<(i64,String)>=sqlx::query_as("SELECT user_id,password_hash FROM singbox_members WHERE email=$1 AND password_hash IS NOT NULL AND (email_verified_at IS NOT NULL OR email=legacy_login_email) AND state IN('active','pending') AND (SELECT enabled FROM singbox_commerce_settings WHERE singleton)").bind(&address).fetch_optional(&state.pool).await?;
    let exists = row.is_some();
    let (id,hash)=row.unwrap_or((-1,"$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRmb3JhcHNw$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into()));
    let permit = state
        .login_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::Busy)?;
    let saved = hash.clone();
    let (valid, upgraded) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let legacy =
            saved.starts_with("$2y$") || saved.starts_with("$2b$") || saved.starts_with("$2a$");
        let valid = if legacy {
            bcrypt::verify(input.password.as_bytes(), &saved).unwrap_or(false)
        } else {
            PasswordHash::new(&saved).is_ok_and(|h| {
                Argon2::default()
                    .verify_password(input.password.as_bytes(), &h)
                    .is_ok()
            })
        };
        let upgraded = if legacy && valid {
            Argon2::default()
                .hash_password(input.password.as_bytes(), &SaltString::generate(&mut OsRng))
                .ok()
                .map(|v| v.to_string())
        } else {
            None
        };
        (valid, upgraded)
    })
    .await
    .map_err(anyhow::Error::from)?;
    if !valid || !exists {
        return Err(ApiError::Unauthorized);
    }
    let mut tx = state.pool.begin().await?;
    enabled(&mut tx).await?;
    ensure_member(&mut tx, id).await?;
    let unchanged:bool=sqlx::query_scalar("SELECT email=$2 AND password_hash=$3 AND (email_verified_at IS NOT NULL OR email=legacy_login_email) AND state IN('active','pending') FROM singbox_members WHERE user_id=$1").bind(id).bind(address).bind(hash).fetch_one(&mut *tx).await?;
    if !unchanged {
        return Err(ApiError::Unauthorized);
    }
    if let Some(hash) = upgraded {
        sqlx::query("UPDATE singbox_members SET password_hash=$2 WHERE user_id=$1")
            .bind(id)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
    }
    let account = account(&mut tx, id).await?;
    let token =
        crate::portal::access::issue(&mut tx, account, crate::portal::access::Proof::Password)
            .await?;
    event(
        &mut tx,
        Some(id),
        "member",
        "password_login",
        &account.to_string(),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    reply(&state, account, &token)
}
async fn account(tx: &mut Tx<'_>, id: i64) -> ApiResult<Uuid> {
    if let Some(account) =
        sqlx::query_scalar("SELECT account_id FROM singbox_portal_accounts WHERE user_id=$1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
    {
        return Ok(account);
    }
    let account = Uuid::new_v4();
    sqlx::query("INSERT INTO passkey_accounts(id) VALUES($1)")
        .bind(account)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO singbox_portal_accounts(user_id,account_id) VALUES($1,$2)")
        .bind(id)
        .bind(account)
        .execute(&mut **tx)
        .await?;
    Ok(account)
}
fn reply(state: &AppState, account: Uuid, token: &str) -> ApiResult<Response> {
    let mut response = crate::passkeys::reply(
        json!({"account_id":account,"url":format!("{}/#/plugins/sing-box/account/{account}",state.config.public_url)}),
    );
    crate::passkeys::set_cookie(state, &mut response, "apsp_proxy_session", token, 86400)?;
    Ok(response)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Reset {
    challenge_id: Uuid,
    code: String,
    password: String,
}
pub(super) async fn reset(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<Reset>,
) -> ApiResult<Response> {
    state.passkeys.check_origin(&headers)?;
    limit(&state, "reset-ip", &peer.ip().to_string(), 20, 600).await?;
    let challenge = check(&state, input.challenge_id, "reset", &input.code, None).await?;
    let hash = password(&state, input.password).await?;
    let id = challenge.user_id.ok_or(ApiError::Unauthorized)?;
    let mut tx = state.pool.begin().await?;
    enabled(&mut tx).await?;
    ensure_member(&mut tx, id).await?;
    let valid: bool = sqlx::query_scalar(
        "SELECT email=$2 AND state IN('active','pending') FROM singbox_members WHERE user_id=$1",
    )
    .bind(id)
    .bind(&challenge.address)
    .fetch_one(&mut *tx)
    .await?;
    if !valid {
        return Err(ApiError::Unauthorized);
    }
    consume(&mut tx, &challenge).await?;
    let recovered: bool = sqlx::query_scalar(
        "SELECT password_hash IS NOT NULL FROM singbox_members WHERE user_id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE singbox_members SET password_hash=$2,email_verified_at=$3,version=version+1 WHERE user_id=$1").bind(id).bind(hash).bind(now_timestamp()).execute(&mut *tx).await?;
    super::admin::revoke_on(&mut tx, id).await?;
    let account = account(&mut tx, id).await?;
    // Recovering an existing password assumes it may have leaked, so passkeys that could
    // have been added with it stop working too; new ones can be added afterwards. Setting
    // a first password leaves passkeys alone, and a password session cannot remove them.
    let mut keys_removed = 0;
    if recovered {
        crate::passkeys::revoke(&mut tx, account).await?;
        keys_removed = sqlx::query("DELETE FROM passkey_credentials WHERE account_id=$1")
            .bind(account)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    }
    let token =
        crate::portal::access::issue(&mut tx, account, crate::portal::access::Proof::Password)
            .await?;
    event(
        &mut tx,
        Some(id),
        "member",
        "password_recovered",
        &id.to_string(),
        json!({"passkeys_removed":keys_removed}),
    )
    .await?;
    tx.commit().await?;
    reply(&state, account, &token)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Password {
    password: String,
}
pub(super) async fn password_change(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<Uuid>,
    Json(input): Json<Password>,
) -> ApiResult<Response> {
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let id = member_session(&mut tx, &state, &headers, account, true).await?;
    let proof = crate::portal::access::session(
        &mut tx,
        account,
        &crate::portal::access::session_hash(&headers)?,
        true,
    )
    .await?;
    let verified:bool=sqlx::query_scalar("SELECT email_verified_at IS NOT NULL OR password_hash IS NOT NULL FROM singbox_members WHERE user_id=$1").bind(id).fetch_one(&mut *tx).await?;
    if !verified {
        return Err(conflict("请先验证联系邮箱，再设置登录密码"));
    }
    let hash = password(&state, input.password).await?;
    sqlx::query("UPDATE singbox_members SET password_hash=$2,version=version+1 WHERE user_id=$1")
        .bind(id)
        .bind(hash)
        .execute(&mut *tx)
        .await?;
    super::admin::revoke_on(&mut tx, id).await?;
    let token = crate::portal::access::issue(&mut tx, account, proof).await?;
    event(
        &mut tx,
        Some(id),
        "member",
        "password_changed",
        &id.to_string(),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    reply(&state, account, &token)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ContactRequest {
    email: String,
}
pub(super) async fn email_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<Uuid>,
    Json(input): Json<ContactRequest>,
) -> ApiResult<Json<Value>> {
    let address = email(&input.email)?;
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let id = member_session(&mut tx, &state, &headers, account, true).await?;
    crate::portal::access::session(
        &mut tx,
        account,
        &crate::portal::access::session_hash(&headers)?,
        true,
    )
    .await?;
    let old: Option<String> =
        sqlx::query_scalar("SELECT email FROM singbox_members WHERE user_id=$1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_members WHERE email=$1 AND user_id<>$2)",
    )
    .bind(&address)
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(conflict("邮箱已被使用"));
    }
    tx.commit().await?;
    limit(&state, "email-change", &id.to_string(), 1, 60).await?;
    let challenge = challenges::send(
        &state,
        "email",
        Some(id),
        &address,
        old.as_deref().filter(|s| *s != address),
        None,
    )
    .await?;
    Ok(Json(
        json!({"challenge_id":challenge,"old_code_required":old.as_deref().is_some_and(|s|s!=address)}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ContactConfirm {
    challenge_id: Uuid,
    code: String,
    old_code: Option<String>,
}
pub(super) async fn email_finish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<Uuid>,
    Json(input): Json<ContactConfirm>,
) -> ApiResult<Response> {
    state.passkeys.check_origin(&headers)?;
    let challenge = check(
        &state,
        input.challenge_id,
        "email",
        &input.code,
        input.old_code.as_deref(),
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let id = member_session(&mut tx, &state, &headers, account, true).await?;
    let proof = crate::portal::access::session(
        &mut tx,
        account,
        &crate::portal::access::session_hash(&headers)?,
        true,
    )
    .await?;
    if challenge.user_id != Some(id) {
        return Err(ApiError::Unauthorized);
    }
    let old: Option<String> =
        sqlx::query_scalar("SELECT email FROM singbox_members WHERE user_id=$1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if old.as_ref() != challenge.old_address.as_ref() && old.as_ref() != Some(&challenge.address) {
        return Err(conflict("当前邮箱已变化，请重新验证"));
    }
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_members WHERE email=$1 AND user_id<>$2)",
    )
    .bind(&challenge.address)
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(conflict("邮箱已被使用"));
    }
    consume(&mut tx, &challenge).await?;
    sqlx::query("UPDATE singbox_members SET email=$2,email_verified_at=$3,version=version+1 WHERE user_id=$1").bind(id).bind(&challenge.address).bind(now_timestamp()).execute(&mut *tx).await?;
    super::admin::revoke_on(&mut tx, id).await?;
    let token = crate::portal::access::issue(&mut tx, account, proof).await?;
    event(
        &mut tx,
        Some(id),
        "member",
        "email_changed",
        &id.to_string(),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    reply(&state, account, &token)
}

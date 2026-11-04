mod reauthentication;

use super::{
    ADMIN_SESSION_SECONDS, LoginRequest, hash_token, proof, random_token, require_admin, security,
    session_cookie,
};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    passkeys::{
        self as keys, ADMIN, ADMIN_BINDING, Ceremony, FinishAuthentication, FinishRegistration,
        PendingAuthentication, PendingRegistration,
    },
};
use apsp_protocol::now_timestamp;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, Path, State},
    http::{HeaderMap, header},
    response::Response,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use sqlx::{PgExecutor, Postgres, Transaction};
use std::net::SocketAddr;
use uuid::Uuid;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/security/passkeys", get(list))
        .route(
            "/api/security/passkeys/register/start",
            post(register_start),
        )
        .route(
            "/api/security/passkeys/register/finish",
            post(register_finish),
        )
        .route("/api/security/passkeys/{id}/remove", post(remove))
        .route("/api/login/passkey/start", post(login_start))
        .route("/api/login/passkey/finish", post(login_finish))
        .route(
            "/api/control-center/reauth/passkey/start",
            post(reauthentication::start),
        )
        .route(
            "/api/control-center/reauth/passkey/finish",
            post(reauthentication::finish),
        )
        .layer(DefaultBodyLimit::max(65536))
}

/// The passkey account of one administrator, if they ever registered a key.
pub(super) async fn account_of<'e>(
    executor: impl PgExecutor<'e>,
    admin: i64,
) -> ApiResult<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT account_id FROM administrator_passkey_accounts WHERE admin_id=$1",
    )
    .bind(admin)
    .fetch_optional(executor)
    .await?)
}

/// Creates the account on an administrator's first registration. The caller
/// holds the administrator row lock, so concurrent first registrations of the
/// same administrator are serialized. The initial owner keeps the account from
/// migration 0039, so keys bound before per-administrator accounts stay valid.
async fn account_for_registration(
    tx: &mut Transaction<'_, Postgres>,
    admin: i64,
) -> ApiResult<Uuid> {
    if let Some(account) = account_of(&mut **tx, admin).await? {
        return Ok(account);
    }
    let account = if admin == 1 { ADMIN } else { Uuid::new_v4() };
    sqlx::query("INSERT INTO passkey_accounts(id) VALUES($1) ON CONFLICT(id) DO NOTHING")
        .bind(account)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO administrator_passkey_accounts(admin_id,account_id) VALUES($1,$2)")
        .bind(admin)
        .bind(account)
        .execute(&mut **tx)
        .await?;
    Ok(account)
}

fn no_keys() -> ApiError {
    ApiError::BadRequest("此账户尚未绑定 Passkey。".into())
}

async fn list(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    let admin = require_admin(&state, &headers).await?;
    let keys = match account_of(&state.pool, admin).await? {
        Some(account) => keys::list(&state.pool, account).await?,
        None => Vec::new(),
    };
    Ok(keys::reply(
        json!({"configuration":state.passkeys.info(), "keys":keys}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Register {
    name: String,
    password: String,
    totp_code: Option<String>,
}

async fn register_start(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<Register>,
) -> ApiResult<Response> {
    let admin = require_admin(&state, &headers).await?;
    let proof = proof::prepare(
        &state,
        &headers,
        peer,
        LoginRequest {
            password: input.password,
            totp_code: input.totp_code,
        },
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    proof.lock(&mut tx).await?;
    let account = account_for_registration(&mut tx, admin).await?;
    let (login_name, display_name): (String, String) = sqlx::query_as(
        "SELECT login_name,display_name FROM administrator_profiles WHERE admin_id=$1",
    )
    .bind(admin)
    .fetch_one(&mut *tx)
    .await?;
    let response = keys::start_registration(
        &state,
        &mut tx,
        account,
        input.name,
        proof.session.clone(),
        ADMIN_BINDING,
        keys::Holder {
            user_name: &login_name,
            display_name: &display_name,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(response)
}

async fn register_finish(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<FinishRegistration>,
) -> ApiResult<Response> {
    let admin = require_admin(&state, &headers).await?;
    let _permit = keys::permit(&state, &headers, peer).await?;
    let session = security::session_hash(&headers)?;
    let account = account_of(&state.pool, admin)
        .await?
        .ok_or_else(keys::invalid)?;
    let pending: Ceremony<PendingRegistration> = keys::consume(
        &state,
        &headers,
        input.challenge_id,
        account,
        "register",
        ADMIN_BINDING,
    )
    .await?;
    if pending.authorization != session {
        return Err(keys::invalid());
    }
    let key = state
        .passkeys
        .registration(input.credential, &pending.state)
        .await?;
    let mut tx = state.pool.begin().await?;
    let row = security::locked_admin(&mut tx, &session).await?;
    if sqlx::Row::try_get::<i64, _>(&row, "id")? != admin {
        return Err(keys::invalid());
    }
    keys::insert(&mut tx, &pending, key).await?;
    tx.commit().await?;
    Ok(keys::reply(json!({"ok":true})))
}

async fn remove(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<LoginRequest>,
) -> ApiResult<Response> {
    let admin = require_admin(&state, &headers).await?;
    let proof = proof::prepare(&state, &headers, peer, input).await?;
    let mut tx = state.pool.begin().await?;
    proof.lock(&mut tx).await?;
    let account = account_of(&mut *tx, admin)
        .await?
        .ok_or(ApiError::NotFound)?;
    keys::lock(&mut tx, account).await?;
    if sqlx::query("DELETE FROM passkey_credentials WHERE id=$1 AND account_id=$2")
        .bind(id)
        .bind(account)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        != 1
    {
        return Err(ApiError::NotFound);
    }
    security::revoke_other_sessions(&mut tx, &proof.session).await?;
    tx.commit().await?;
    Ok(keys::reply(json!({"ok":true})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginStart {
    #[serde(default)]
    login_name: Option<String>,
}

/// Same default as password login: an empty request means the initial owner.
fn requested_login(body: &[u8]) -> ApiResult<String> {
    let name = if body.iter().all(u8::is_ascii_whitespace) {
        None
    } else {
        serde_json::from_slice::<LoginStart>(body)
            .map_err(|_| ApiError::BadRequest("登录请求格式无效。".into()))?
            .login_name
    };
    let name = name.unwrap_or_else(|| "admin".into());
    if name.is_empty() || name.len() > 100 {
        return Err(no_keys());
    }
    Ok(name)
}

async fn login_start(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let _permit = keys::permit(&state, &headers, peer).await?;
    let login_name = requested_login(&body)?;
    let mut tx = state.pool.begin().await?;
    // Only keys of the named, enabled administrator are offered. An unknown
    // name and a name without keys get the same answer.
    let account: Uuid = sqlx::query_scalar("SELECT k.account_id FROM administrator_profiles p JOIN administrator_passkey_accounts k ON k.admin_id=p.admin_id WHERE p.login_name=$1 AND p.enabled")
        .bind(&login_name).fetch_optional(&mut *tx).await?.ok_or_else(no_keys)?;
    let response = keys::start_authentication(&state, &mut tx, account, ADMIN_BINDING).await?;
    tx.commit().await?;
    Ok(response)
}

async fn login_finish(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<FinishAuthentication>,
) -> ApiResult<Response> {
    let result = login_finish_inner(state.clone(), peer, headers, input).await;
    if result.is_err()
        && let Err(error)=sqlx::query("INSERT INTO management_audit(action,object_path,request_diff,result,occurred_at) VALUES('login','/api/login/passkey/finish',$1,$2,$3)")
            .bind(json!({"factor":"passkey"})).bind(json!({"phase":"authentication-denied","success":false})).bind(now_timestamp()).execute(&state.pool).await {
            tracing::warn!(%error,"passkey denial audit failed");
    }
    result
}

async fn login_finish_inner(
    state: AppState,
    peer: SocketAddr,
    headers: HeaderMap,
    input: FinishAuthentication,
) -> ApiResult<Response> {
    let _permit = keys::permit(&state, &headers, peer).await?;
    // The challenge names the account it was issued for; only an administrator
    // account can become an administrator session.
    let account: Uuid = sqlx::query_scalar("SELECT c.account_id FROM passkey_ceremonies c JOIN administrator_passkey_accounts k ON k.account_id=c.account_id WHERE c.id=$1 AND c.purpose='authenticate'")
        .bind(input.challenge_id).fetch_optional(&state.pool).await?.ok_or_else(keys::invalid)?;
    let pending: Ceremony<PendingAuthentication> = keys::consume(
        &state,
        &headers,
        input.challenge_id,
        account,
        "authenticate",
        ADMIN_BINDING,
    )
    .await?;
    let result = state
        .passkeys
        .authentication(input.credential, &pending.state)
        .await?;
    let token = random_token();
    let mut tx = state.pool.begin().await?;
    let admin: i64 = sqlx::query_scalar("SELECT a.id FROM administrator_passkey_accounts k JOIN admins a ON a.id=k.admin_id JOIN administrator_profiles p ON p.admin_id=a.id WHERE k.account_id=$1 AND p.enabled FOR UPDATE OF a,p")
        .bind(account).fetch_optional(&mut *tx).await?.ok_or(ApiError::Unauthorized)?;
    keys::authenticate(&mut tx, &pending, result).await?;
    let now = now_timestamp();
    sqlx::query("DELETE FROM sessions WHERE expires_at <= $1")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO sessions(token_hash,admin_id,expires_at) VALUES($1,$2,$3)")
        .bind(hash_token(&token))
        .bind(admin)
        .bind(now + ADMIN_SESSION_SECONDS)
        .execute(&mut *tx)
        .await?;
    proof::record_recent(&mut tx, &hash_token(&token)).await?;
    sqlx::query("INSERT INTO management_audit(admin_id,action,object_path,request_diff,result,occurred_at) VALUES($1,'login','/api/login/passkey/finish',$2,$3,$4)")
        .bind(admin).bind(json!({"factor":"passkey"})).bind(json!({"phase":"authenticated","success":true})).bind(now).execute(&mut *tx).await?;
    tx.commit().await?;
    let mut response = keys::reply(json!({"id":admin}));
    response.headers_mut().insert(
        header::SET_COOKIE,
        session_cookie(&state, &token, ADMIN_SESSION_SECONDS)?,
    );
    keys::set_cookie(&state, &mut response, ADMIN_BINDING, "", 0)?;
    Ok(response)
}

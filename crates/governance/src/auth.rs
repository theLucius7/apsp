use crate::{
    AppState,
    control_center::audit,
    error::{ApiError, ApiResult},
};
use apsp_panel_host::{cookies, passwords};
use apsp_protocol::now_timestamp;
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::net::SocketAddr;

pub(crate) mod passkeys;
pub mod proof;
pub(crate) mod rate_limit;
pub(crate) mod security;
mod totp;

pub use crate::tokens::{hash_token, random_token};
pub use security::{
    password_change, session_hash, totp_confirm, totp_disable, totp_setup, totp_status,
};

const ADMIN_SESSION_SECONDS: i64 = 86_400;
/// The cookie that carries an administrator's session token.
pub(crate) const SESSION_COOKIE: &str = "apsp_session";

pub async fn ensure_admin(pool: &PgPool, password: Option<&str>) -> anyhow::Result<()> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admins WHERE id = 1)")
        .fetch_one(pool)
        .await?;
    if exists {
        return Ok(());
    }
    let password = password
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("APSP_ADMIN_PASSWORD is required for the initial administrator")
        })?
        .to_owned();
    if password.len() > 1024 {
        anyhow::bail!("initial administrator password exceeds 1024 bytes");
    }
    let hash = hash_password(password).await?;
    sqlx::query(
        "INSERT INTO admins (id, password_hash) VALUES (1, $1) ON CONFLICT (id) DO NOTHING",
    )
    .bind(hash)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO administrator_profiles(admin_id,login_name,display_name,role,all_servers,created_at,updated_at) VALUES(1,'admin','所有者','owner',true,$1,$1) ON CONFLICT(admin_id) DO NOTHING")
        .bind(now_timestamp()).execute(pool).await?;
    Ok(())
}

/// Hashes an administrator password on a blocking thread.
pub(crate) async fn hash_password(password: String) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || passwords::hash(password.as_bytes())).await?
}

/// Removes expired administrator sessions without waiting for the next login. Device
/// sessions are the resources domain's and expire there.
pub async fn purge_expired_sessions(pool: &PgPool, now: i64) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE expires_at <= $1")
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn require_admin(state: &AppState, headers: &HeaderMap) -> ApiResult<i64> {
    Ok(crate::control_center::authenticate(state, headers)
        .await?
        .admin_id)
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub password: String,
    #[serde(default)]
    pub totp_code: Option<String>,
}

pub(crate) async fn verified_admin_password(
    state: &AppState,
    admin_id: i64,
    password: String,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> ApiResult<(Option<String>, tokio::sync::OwnedSemaphorePermit)> {
    if password.is_empty() || password.len() > 1024 {
        return Ok((None, permit));
    }
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM admins WHERE id = $1")
        .bind(admin_id)
        .fetch_one(&state.pool)
        .await?;
    let expected = hash.clone();
    let (verified, permit) = tokio::task::spawn_blocking(move || {
        (passwords::verify(password.as_bytes(), &expected), permit)
    })
    .await
    .map_err(anyhow::Error::from)?;
    Ok((verified.then_some(hash), permit))
}

#[derive(Deserialize)]
pub struct NamedLoginRequest {
    #[serde(default)]
    pub login_name: Option<String>,
    #[serde(flatten)]
    pub credentials: LoginRequest,
}

pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(request): Json<NamedLoginRequest>,
) -> ApiResult<Response> {
    let login_name = request
        .login_name
        .as_deref()
        .unwrap_or("admin")
        .chars()
        .take(100)
        .collect::<String>();
    let result = login_inner(state.clone(), peer, request).await;
    if result.is_err()
        && let Err(error) = audit::record(
            &state.pool,
            audit::Entry {
                admin_id: None,
                token_id: None,
                action: "login",
                object_path: "/api/login",
                request_diff: json!({"login_name":login_name,"factor":"password"}),
                result: json!({"phase":"authentication-denied","success":false}),
                occurred_at: now_timestamp(),
            },
        )
        .await
    {
        tracing::warn!(%error, "authentication denial audit failed");
    }
    result
}

async fn login_inner(
    state: AppState,
    peer: SocketAddr,
    request: NamedLoginRequest,
) -> ApiResult<Response> {
    let permit = state
        .login_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::Busy)?;
    rate_limit::consume(&state.pool, peer).await?;
    let login_name = request.login_name.as_deref().unwrap_or("admin");
    if login_name.is_empty() || login_name.len() > 100 {
        return Err(ApiError::Unauthorized);
    }
    let admin_id: i64 = sqlx::query_scalar(
        "SELECT admin_id FROM administrator_profiles WHERE login_name=$1 AND enabled",
    )
    .bind(login_name)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    let request = request.credentials;
    let (verified_hash, _permit) =
        verified_admin_password(&state, admin_id, request.password, permit).await?;
    let verified_hash = verified_hash.ok_or(ApiError::Unauthorized)?;
    let token = random_token();
    let mut transaction = state.pool.begin().await?;
    let admin = sqlx::query(
        "SELECT a.password_hash,a.totp_secret,a.totp_last_step FROM admins a JOIN administrator_profiles p ON p.admin_id=a.id WHERE a.id=$1 AND p.enabled FOR UPDATE OF a,p",
    )
    .bind(admin_id)
    .fetch_optional(&mut *transaction)
    .await?.ok_or(ApiError::Unauthorized)?;
    let now = now_timestamp();
    if admin.try_get::<String, _>("password_hash")? != verified_hash {
        return Err(ApiError::Unauthorized);
    }
    if let Some(secret) = admin.try_get::<Option<Vec<u8>>, _>("totp_secret")? {
        let step = totp::verify(
            &secret,
            request.totp_code.as_deref().unwrap_or(""),
            now,
            admin.try_get("totp_last_step")?,
        )
        .ok_or(ApiError::Unauthorized)?;
        sqlx::query("UPDATE admins SET totp_last_step = $1 WHERE id = $2")
            .bind(step)
            .bind(admin_id)
            .execute(&mut *transaction)
            .await?;
    }
    sqlx::query("DELETE FROM sessions WHERE expires_at <= $1")
        .bind(now)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO sessions (token_hash, admin_id, expires_at) VALUES ($1, $2, $3)")
        .bind(hash_token(&token))
        .bind(admin_id)
        .bind(now + ADMIN_SESSION_SECONDS)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "INSERT INTO administrator_reauth(session_hash,verified_at,expires_at) VALUES($1,$2,$3)",
    )
    .bind(hash_token(&token))
    .bind(now)
    .bind(now + 300)
    .execute(&mut *transaction)
    .await?;
    audit::record(
        &mut *transaction,
        audit::Entry {
            admin_id: Some(admin_id),
            token_id: None,
            action: "login",
            object_path: "/api/login",
            request_diff: json!({"factor":"password"}),
            result: json!({"phase":"authenticated","success":true}),
            occurred_at: now,
        },
    )
    .await?;
    transaction.commit().await?;
    let mut response = Json(json!({"id": admin_id})).into_response();
    cookies::set(
        &state,
        &mut response,
        SESSION_COOKIE,
        &token,
        ADMIN_SESSION_SECONDS,
    )?;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(token) = cookies::read(&headers, SESSION_COOKIE) {
        let mut tx = state.pool.begin().await?;
        let admin: Option<i64> =
            sqlx::query_scalar("DELETE FROM sessions WHERE token_hash = $1 RETURNING admin_id")
                .bind(hash_token(token))
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(admin) = admin {
            audit::record(
                &mut *tx,
                audit::Entry {
                    admin_id: Some(admin),
                    token_id: None,
                    action: "logout",
                    object_path: "/api/logout",
                    request_diff: json!({}),
                    result: json!({"phase":"session-revoked","success":true}),
                    occurred_at: now_timestamp(),
                },
            )
            .await?;
        }
        tx.commit().await?;
    }
    let mut response = Json(json!({"ok": true})).into_response();
    cookies::set(&state, &mut response, SESSION_COOKIE, "", 0)?;
    Ok(response)
}

pub async fn me(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let actor = crate::control_center::authenticate(&state, &headers).await?;
    Ok(Json(json!({"id": actor.admin_id,"administrator":actor})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "../panel/migrations")]
    async fn expired_sessions_are_purged_and_live_sessions_kept(
        pool: PgPool,
    ) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO admins(id,password_hash) VALUES(1,'TEST_ONLY'),(2,'TEST_ONLY')")
            .execute(&pool)
            .await?;
        for (token, admin, expires_at) in [
            ("expired", 1_i64, 100_i64),
            ("expired-now", 2, 300),
            ("live", 1, 301),
            ("live-other", 2, 400),
        ] {
            sqlx::query("INSERT INTO sessions(token_hash,admin_id,expires_at) VALUES($1,$2,$3)")
                .bind(hash_token(token))
                .bind(admin)
                .bind(expires_at)
                .execute(&pool)
                .await?;
        }
        purge_expired_sessions(&pool, 300).await?;
        let mut remaining: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM sessions")
            .fetch_all(&pool)
            .await?;
        remaining.sort();
        let mut expected = vec![hash_token("live"), hash_token("live-other")];
        expected.sort();
        assert_eq!(remaining, expected);
        Ok(())
    }
}

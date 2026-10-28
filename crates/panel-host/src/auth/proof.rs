use super::{LoginRequest, require_admin, security, totp, verified_admin_password};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    passkeys,
};
use apsp_protocol::now_timestamp;
use axum::http::HeaderMap;
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use std::net::SocketAddr;

// Call only after authenticating and locking the current administrator session,
// or inserting a new authenticated session in this transaction.
pub(crate) async fn record_recent(
    tx: &mut Transaction<'_, Postgres>,
    session: &str,
) -> ApiResult<Value> {
    let now = now_timestamp();
    let expires_at = now + 300;
    sqlx::query("INSERT INTO administrator_reauth(session_hash,verified_at,expires_at) VALUES($1,$2,$3) ON CONFLICT(session_hash) DO UPDATE SET verified_at=EXCLUDED.verified_at,expires_at=EXCLUDED.expires_at")
        .bind(session).bind(now).bind(expires_at).execute(&mut **tx).await?;
    Ok(json!({"verified_at":now,"expires_at":expires_at}))
}

pub struct Proof {
    pub session: String,
    hash: String,
    code: Option<String>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

pub async fn prepare(
    state: &AppState,
    headers: &HeaderMap,
    peer: SocketAddr,
    input: LoginRequest,
) -> ApiResult<Proof> {
    let admin_id = require_admin(state, headers).await?;
    let session = security::session_hash(headers)?;
    let permit = passkeys::password_permit(state, headers, peer).await?;
    let (hash, permit) = verified_admin_password(state, admin_id, input.password, permit).await?;
    Ok(Proof {
        session,
        hash: hash.ok_or_else(|| ApiError::BadRequest("管理员密码或验证码不正确。".into()))?,
        code: input.totp_code,
        _permit: permit,
    })
}

impl Proof {
    pub async fn lock(&self, tx: &mut Transaction<'_, Postgres>) -> ApiResult<()> {
        let row = security::locked_admin(tx, &self.session).await?;
        if row.try_get::<String, _>("password_hash")? != self.hash {
            return Err(ApiError::Unauthorized);
        }
        if let Some(secret) = row.try_get::<Option<Vec<u8>>, _>("totp_secret")? {
            let step = totp::verify(
                &secret,
                self.code.as_deref().unwrap_or(""),
                now_timestamp(),
                row.try_get("totp_last_step")?,
            )
            .ok_or_else(|| {
                ApiError::BadRequest("管理员密码或验证码不正确、已过期或已使用。".into())
            })?;
            sqlx::query("UPDATE admins SET totp_last_step=$1 WHERE id=$2")
                .bind(step)
                .bind(row.try_get::<i64, _>("id")?)
                .execute(&mut **tx)
                .await?;
        }
        Ok(())
    }
}

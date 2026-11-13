//! Self-service accounts of proxy users: email and password sign-in, contact
//! email, reminders, guides and invitations (ADR 0113). Selling, wallets and
//! payments from the migrated member store are gone; their tables stay only as
//! history and for the compatibility bridge.
mod accounts;
mod admin;
mod guides;
mod invitations;
mod mail;
mod messages;

use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use axum::{
    Router,
    http::HeaderMap,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

type Tx<'a> = Transaction<'a, Postgres>;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/access/account-mail",
            get(mail::settings).put(mail::save),
        )
        .route(
            "/api/access/portal-guides",
            get(guides::list).post(guides::create),
        )
        .route(
            "/api/access/portal-guides/{id}",
            axum::routing::put(guides::update).delete(guides::remove),
        )
        .route(
            "/api/access/invitation-settings",
            get(invitations::settings).put(invitations::settings_save),
        )
        .route("/api/access/users/{id}/invitations", get(invitations::user))
        .route(
            "/api/access/users/{id}/invitations/quota",
            post(invitations::adjust),
        )
        .route("/api/access/portal/{account}/account", get(admin::portal))
        .route(
            "/api/access/portal/{account}/account/password",
            post(accounts::password_change),
        )
        .route(
            "/api/access/portal/{account}/account/email/start",
            post(accounts::email_start),
        )
        .route(
            "/api/access/portal/{account}/account/email/finish",
            post(accounts::email_finish),
        )
        .route(
            "/api/access/portal/{account}/account/messages",
            get(messages::list),
        )
        .route(
            "/api/access/portal/{account}/account/messages/read",
            post(messages::read),
        )
        .route(
            "/api/access/portal/{account}/account/reminders",
            axum::routing::put(messages::preferences),
        )
        .route(
            "/api/access/portal/{account}/invitations",
            post(invitations::create),
        )
        .route(
            "/api/access/portal/{account}/invitations/{id}/rotate",
            post(invitations::rotate),
        )
        .route(
            "/api/access/portal/{account}/invitations/{id}/revoke",
            post(invitations::revoke),
        )
        .route(
            "/api/access/portal-public/account/login",
            post(accounts::login),
        )
        .route(
            "/api/access/portal-public/account/challenge",
            post(accounts::challenge),
        )
        .route(
            "/api/access/portal-public/account/reset",
            post(accounts::reset),
        )
        .route(
            "/api/access/portal-public/invitation",
            post(invitations::inspect),
        )
        .route("/api/access/portal-public/claim", post(invitations::claim))
}

fn bad(message: &str) -> ApiError {
    ApiError::BadRequest(message.into())
}
fn conflict(message: &str) -> ApiError {
    ApiError::Conflict(message.into())
}
fn text(value: &str, maximum: usize, required: bool) -> ApiResult<String> {
    let value = value.trim();
    if (required && value.is_empty())
        || value.chars().count() > maximum
        || value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(bad("文字长度或格式无效"));
    }
    Ok(value.into())
}
fn email(value: &str) -> ApiResult<String> {
    let value = value.trim().to_lowercase();
    let parts: Vec<_> = value.split('@').collect();
    if value.len() > 254
        || parts.len() != 2
        || parts[0].is_empty()
        || !parts[1].contains('.')
        || value.chars().any(char::is_whitespace)
        || !value.is_ascii()
    {
        return Err(bad("请填写有效的联系邮箱"));
    }
    Ok(value)
}
async fn administrator(state: &AppState, headers: &HeaderMap, write: bool) -> ApiResult<String> {
    let id = crate::control_center::require_capability(
        state,
        headers,
        if write { "proxy:write" } else { "proxy:read" },
    )
    .await?;
    if write {
        crate::control_center::require_recent_proof(state, headers).await?;
    }
    Ok(format!("admin:{id}"))
}
/// A package assigned outside the member store must still be translatable by an
/// enabled compatibility bridge, or every later synchronization would fail.
pub(crate) async fn require_compatible_package(
    tx: &mut Tx<'_>,
    user: i64,
    package: i64,
) -> ApiResult<()> {
    let enrolled: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_members WHERE user_id=$1 AND state<>'staged')",
    )
    .bind(user)
    .fetch_one(&mut **tx)
    .await?;
    let missing = crate::nexus::bridge::package_unmapped(tx, package).await?;
    if enrolled && missing {
        return Err(conflict(
            "此套餐尚未配置原节点权限映射，不能分配给兼容通道中的成员",
        ));
    }
    Ok(())
}
/// The sign-in profile of a proxy user. While the compatibility bridge runs, a new
/// profile would also enrol the user into it and move their subscription to the
/// retained renderer, so only migrated and invited users get one then.
async fn ensure_profile(tx: &mut Tx<'_>, id: i64) -> ApiResult<()> {
    crate::proxy_users::lock_user(tx, id).await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_members WHERE user_id=$1)")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    if exists {
        return Ok(());
    }
    let bridged = crate::nexus::bridge::enabled(tx).await?;
    if bridged {
        return Err(conflict(
            "原系统兼容期间，此账号请使用 Passkey 登录；邮箱和密码在兼容结束后开放",
        ));
    }
    create_profile(tx, id, None, None).await
}
/// Creates the profile row; the bridge still reads a wallet row for every member.
async fn create_profile(
    tx: &mut Tx<'_>,
    id: i64,
    email: Option<&str>,
    inviter: Option<i64>,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO singbox_members(user_id,email,inviter_id,created_at) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
        .bind(id)
        .bind(email)
        .bind(inviter)
        .bind(now_timestamp())
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO singbox_wallets(user_id) VALUES($1) ON CONFLICT DO NOTHING")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
/// The proxy user behind a portal session. A write needs a profile to write to.
async fn account_session(
    tx: &mut Tx<'_>,
    state: &AppState,
    headers: &HeaderMap,
    account: Uuid,
    write: bool,
) -> ApiResult<i64> {
    if write {
        crate::subscription_hosts::check_portal_origin(state, headers).await?;
    }
    let owner = crate::portal::access::lock(tx, account).await?;
    crate::portal::access::session(
        tx,
        account,
        &crate::portal::access::session_hash(headers)?,
        false,
    )
    .await?;
    let id: i64 = owner.try_get("user_id")?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT state FROM singbox_members WHERE user_id=$1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    if status
        .as_deref()
        .is_some_and(|status| !["active", "pending"].contains(&status))
    {
        return Err(ApiError::Forbidden("此账号暂不可用".into()));
    }
    Ok(id)
}
async fn event(
    tx: &mut Tx<'_>,
    user: Option<i64>,
    actor: &str,
    action: &str,
    object: &str,
    detail: Value,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO singbox_commerce_events(user_id,actor,action,object_id,detail,created_at) VALUES($1,$2,$3,$4,$5,$6)").bind(user).bind(actor).bind(action).bind(object).bind(detail).bind(now_timestamp()).execute(&mut **tx).await?;
    Ok(())
}

async fn list_json(tx: &mut Tx<'_>, sql: &str, id: i64) -> ApiResult<Value> {
    let values: Vec<Value> = sqlx::query_scalar(sql)
        .bind(id)
        .fetch_all(&mut **tx)
        .await?;
    Ok(json!(values))
}

pub(super) async fn run(state: AppState) {
    messages::run(state).await;
}

/// Ends every portal session of a user, after a password or email change.
async fn revoke_sessions(tx: &mut Tx<'_>, id: i64) -> ApiResult<()> {
    sqlx::query("DELETE FROM singbox_portal_sessions WHERE account_id IN(SELECT account_id FROM singbox_portal_accounts WHERE user_id=$1)").bind(id).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM passkey_ceremonies WHERE account_id IN(SELECT account_id FROM singbox_portal_accounts WHERE user_id=$1)").bind(id).execute(&mut **tx).await?;
    Ok(())
}

mod accounts;
mod admin;
mod catalog;
mod compatibility;
mod content;
mod invitations;
mod mail;
mod messages;
pub mod migration;
mod orders;
mod payments;
mod wallet;

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
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

type Tx<'a> = Transaction<'a, Postgres>;
const MAX_BALANCE: i64 = 2_147_483_647;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/commerce/mail", get(mail::settings).put(mail::save))
        .route(
            "/commerce/settings",
            get(admin::settings).put(admin::settings_save),
        )
        .route("/commerce/overview", get(admin::overview))
        .route("/commerce/members", get(admin::members))
        .route("/commerce/members/bulk", post(admin::bulk))
        .route(
            "/commerce/members/{id}",
            get(admin::member).put(admin::member_save),
        )
        .route("/commerce/members/{id}/adjust", post(wallet::adjust))
        .route(
            "/commerce/members/{id}/sessions/revoke",
            post(admin::revoke_sessions),
        )
        .route("/commerce/offers", get(catalog::list).post(catalog::create))
        .route("/commerce/offers/{id}", axum::routing::put(catalog::update))
        .route("/commerce/orders", get(orders::list))
        .route("/commerce/invitations", get(invitations::list))
        .route(
            "/commerce/content",
            get(content::list).post(content::create),
        )
        .route(
            "/commerce/content/{id}",
            axum::routing::put(content::update),
        )
        .route(
            "/commerce/channels",
            get(payments::channels).post(payments::channel_create),
        )
        .route(
            "/commerce/channels/{id}",
            axum::routing::put(payments::channel_update),
        )
        .route("/commerce/payments", get(payments::list))
        .route("/commerce/exceptions/{id}/resolve", post(payments::resolve))
        .route(
            "/commerce/migration/preview",
            post(migration::preview).layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route(
            "/commerce/migration/import",
            post(migration::import).layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route("/commerce/migration", get(migration::history))
        .route("/commerce/migration/records", get(migration::records))
        .route(
            "/commerce/compatibility",
            get(compatibility::status).post(compatibility::activate),
        )
        .route("/portal/{account}/commerce", get(admin::portal))
        .route(
            "/portal/{account}/commerce/purchase",
            post(orders::purchase),
        )
        .route(
            "/portal/{account}/commerce/invitations",
            post(invitations::create),
        )
        .route(
            "/portal/{account}/commerce/invitations/{id}/rotate",
            post(invitations::rotate),
        )
        .route(
            "/portal/{account}/commerce/invitations/{id}/revoke",
            post(invitations::revoke),
        )
        .route("/portal/{account}/commerce/topups", post(payments::topup))
        .route(
            "/portal/{account}/commerce/topups/{id}/checkout",
            post(payments::checkout),
        )
        .route(
            "/portal/{account}/commerce/topups/{id}/cancel",
            post(payments::cancel),
        )
        .route("/commerce-public/account/login", post(accounts::login))
        .route(
            "/commerce-public/account/challenge",
            post(accounts::challenge),
        )
        .route("/commerce-public/account/reset", post(accounts::reset))
        .route(
            "/portal/{account}/commerce/password",
            post(accounts::password_change),
        )
        .route(
            "/portal/{account}/commerce/email/start",
            post(accounts::email_start),
        )
        .route(
            "/portal/{account}/commerce/email/finish",
            post(accounts::email_finish),
        )
        .route("/portal/{account}/commerce/messages", get(messages::list))
        .route(
            "/portal/{account}/commerce/messages/read",
            post(messages::read),
        )
        .route(
            "/portal/{account}/commerce/reminders",
            axum::routing::put(messages::preferences),
        )
        .route(
            "/portal/{account}/commerce/remind-sponsor",
            post(messages::remind),
        )
        .route("/commerce-public/invitation", post(invitations::inspect))
        .route("/commerce-public/claim", post(invitations::claim))
        .route(
            "/commerce-public/epay/{id}",
            get(payments::callback_get).post(payments::callback_post),
        )
}

fn bad(message: &str) -> ApiError {
    ApiError::BadRequest(message.into())
}
fn conflict(message: &str) -> ApiError {
    ApiError::Conflict(message.into())
}
fn fingerprint(value: &impl Serialize) -> ApiResult<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(anyhow::Error::from)?)
    ))
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
async fn enabled(tx: &mut Tx<'_>) -> ApiResult<()> {
    let enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM singbox_commerce_settings WHERE singleton")
            .fetch_one(&mut **tx)
            .await?;
    if !enabled {
        return Err(ApiError::Forbidden("成员运营尚未启用".into()));
    }
    Ok(())
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
    let missing:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_nexus_bridge b WHERE b.enabled AND NOT EXISTS(SELECT 1 FROM singbox_nexus_plan_mappings p WHERE p.import_id=b.import_id AND p.package_group_id=$1 AND p.legacy_group_id IS NOT NULL))").bind(package).fetch_one(&mut **tx).await?;
    if enrolled && missing {
        return Err(conflict(
            "此套餐尚未配置原节点权限映射，不能分配给兼容通道中的成员",
        ));
    }
    Ok(())
}
async fn ensure_member(tx: &mut Tx<'_>, id: i64) -> ApiResult<()> {
    crate::business::lock_user(tx, id).await?;
    sqlx::query(
        "INSERT INTO singbox_members(user_id,created_at) VALUES($1,$2) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(now_timestamp())
    .execute(&mut **tx)
    .await?;
    sqlx::query("INSERT INTO singbox_wallets(user_id) VALUES($1) ON CONFLICT DO NOTHING")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn member_session(
    tx: &mut Tx<'_>,
    state: &AppState,
    headers: &HeaderMap,
    account: Uuid,
    write: bool,
) -> ApiResult<i64> {
    if write {
        state.passkeys.check_origin(headers)?;
    }
    enabled(tx).await?;
    let owner = crate::portal::access::lock(tx, account).await?;
    crate::portal::access::session(
        tx,
        account,
        &crate::portal::access::session_hash(headers)?,
        false,
    )
    .await?;
    let id: i64 = owner.try_get("user_id")?;
    // Only a member action creates the member row. Merely viewing the portal must not
    // enroll an ordinary proxy user into the compatibility bridge and its packages.
    if write {
        ensure_member(tx, id).await?;
    }
    let status: Option<String> =
        sqlx::query_scalar("SELECT state FROM singbox_members WHERE user_id=$1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    let status = status.unwrap_or_else(|| "active".into());
    if !["active", "pending"].contains(&status.as_str()) {
        return Err(ApiError::Forbidden("此成员账户暂不可用".into()));
    }
    Ok(id)
}
async fn active(tx: &mut Tx<'_>, user: i64) -> ApiResult<()> {
    let allowed: bool=sqlx::query_scalar("SELECT state='active' AND NOT(sponsor_required AND first_purchase_at IS NULL) FROM singbox_members WHERE user_id=$1").bind(user).fetch_one(&mut **tx).await?;
    if !allowed {
        return Err(ApiError::Forbidden(
            "首次开通请联系邀请人，开通后可自行购买".into(),
        ));
    }
    Ok(())
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

#[derive(Default, Deserialize)]
pub(super) struct Page {
    #[serde(default)]
    offset: u32,
}
impl Page {
    fn offset(&self) -> i64 {
        i64::from(self.offset.min(1_000_000))
    }
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

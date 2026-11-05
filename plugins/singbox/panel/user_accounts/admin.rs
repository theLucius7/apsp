use super::*;
use axum::{
    Json,
    extract::{Path, State},
};

/// What a signed-in proxy user sees about their own account: sign-in profile,
/// reminders, published guides with their subscription filled in, and invitations.
pub(super) async fn portal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    let user = account_session(&mut tx, &state, &headers, account, false).await?;
    // A proxy user who never set an email or password has no profile; show defaults.
    let profile:Value=sqlx::query_scalar("SELECT jsonb_build_object('user_id',u.id,'email',m.email,'email_verified',COALESCE(m.email_verified_at IS NOT NULL,FALSE),'has_password',COALESCE(m.password_hash IS NOT NULL,FALSE),'remind_expire',COALESCE(m.remind_expire,TRUE),'remind_traffic',COALESCE(m.remind_traffic,TRUE)) FROM users u LEFT JOIN singbox_members m ON m.user_id=u.id WHERE u.id=$1").bind(user).fetch_one(&mut *tx).await?;
    let mut guides:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(c) FROM singbox_member_content c WHERE published ORDER BY updated_at DESC,id DESC LIMIT 500").fetch_all(&mut *tx).await?;
    let allowed: bool =
        sqlx::query_scalar("SELECT allowed FROM singbox_entitlements($1) WHERE user_id=$2")
            .bind(now_timestamp())
            .bind(user)
            .fetch_one(&mut *tx)
            .await?;
    let (token,base):(String,Option<String>)=sqlx::query_as("SELECT u.subscription_token,b.subscription_base FROM users u LEFT JOIN singbox_nexus_identities i ON i.user_id=u.id LEFT JOIN singbox_nexus_bridge b ON b.import_id=i.import_id AND b.enabled WHERE u.id=$1").bind(user).fetch_one(&mut *tx).await?;
    let own_host = super::super::subscription_hosts::own_address(&state.pool, user).await?;
    let url = match (own_host, base) {
        (Some(host), Some(_)) => host,
        (Some(host), None) => format!("{host}?format=singbox"),
        (None, Some(base)) => format!("{base}{token}"),
        (None, None) => format!("{}/sub/{token}?format=singbox", state.config.public_url),
    };
    for entry in &mut guides {
        if let Some(body) = entry["body"].as_str() {
            entry["body"] = json!(guides::render(body, allowed, &url));
        }
    }
    let invitations = invitations::summary(&mut tx, user).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"profile":profile,"guides":guides,"invitations":invitations}),
    ))
}

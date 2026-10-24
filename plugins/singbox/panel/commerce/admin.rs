use super::*;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};

pub(super) async fn settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    administrator(&state, &headers, false).await?;
    Ok(Json(
        sqlx::query_scalar(
            "SELECT to_jsonb(s)-'singleton' FROM singbox_commerce_settings s WHERE singleton",
        )
        .fetch_one(&state.pool)
        .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    enabled: bool,
    invitations_enabled: bool,
    invite_price_cents: i64,
    version: i64,
}
pub(super) async fn settings_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Settings>,
) -> ApiResult<Json<Value>> {
    let actor = administrator(&state, &headers, true).await?;
    if !(1..=100_000_000).contains(&input.invite_price_cents) {
        return Err(bad("邀请价格须为有效的正整数分"));
    }
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let value:Value=sqlx::query_scalar("UPDATE singbox_commerce_settings SET enabled=$1,invitations_enabled=$2,invite_price_cents=$3,version=version+1,updated_at=$4 WHERE singleton AND version=$5 RETURNING to_jsonb(singbox_commerce_settings)-'singleton'").bind(input.enabled).bind(input.invitations_enabled).bind(input.invite_price_cents).bind(now_timestamp()).bind(input.version).fetch_optional(&mut *tx).await?.ok_or_else(|| conflict("运营设置已变化，请刷新"))?;
    event(
        &mut tx,
        None,
        &actor,
        "settings_saved",
        "settings",
        value.clone(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(value))
}
pub(super) async fn overview(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    administrator(&state, &headers, false).await?;
    Ok(Json(sqlx::query_scalar("SELECT jsonb_build_object('members',(SELECT count(*) FROM users WHERE deleted_at IS NULL),'pending_members',(SELECT count(*) FROM singbox_members WHERE state='pending'),'balance_cents',(SELECT COALESCE(sum(balance_cents),0)::text FROM singbox_wallets),'orders',(SELECT count(*) FROM singbox_commerce_orders),'sales_cents',(SELECT COALESCE(sum(amount_cents),0)::text FROM singbox_commerce_orders WHERE state='fulfilled'),'open_exceptions',(SELECT count(*) FROM singbox_payment_exceptions WHERE state='open'),'invitations',(SELECT count(*) FROM singbox_commerce_invitations),'ledger_entries',(SELECT count(*) FROM singbox_wallet_ledger))").fetch_one(&state.pool).await?))
}
pub(super) async fn members(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    administrator(&state, &headers, false).await?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('user_id',u.id,'name',u.name,'email',m.email,'state',COALESCE(m.state,'active'),'version',COALESCE(m.version,0),'balance_cents',COALESCE(w.balance_cents,0),'inviter_id',m.inviter_id,'sponsor_required',COALESCE(m.sponsor_required,FALSE),'account_id',p.account_id) FROM users u LEFT JOIN singbox_members m ON m.user_id=u.id LEFT JOIN singbox_wallets w ON w.user_id=u.id LEFT JOIN singbox_portal_accounts p ON p.user_id=u.id WHERE u.deleted_at IS NULL ORDER BY u.id LIMIT 100 OFFSET $1").bind(page.offset()).fetch_all(&state.pool).await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE deleted_at IS NULL")
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(
        json!({"entries":rows,"total":total,"offset":page.offset()}),
    ))
}
pub(super) async fn member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    administrator(&state, &headers, false).await?;
    let mut tx = state.pool.begin().await?;
    ensure_member(&mut tx, id).await?;
    let profile:Value=sqlx::query_scalar("SELECT (to_jsonb(m)-'password_hash')||jsonb_build_object('name',u.name,'balance_cents',w.balance_cents) FROM singbox_members m JOIN users u ON u.id=m.user_id JOIN singbox_wallets w ON w.user_id=m.user_id WHERE m.user_id=$1").bind(id).fetch_one(&mut *tx).await?;
    let history = history(&mut tx, id).await?;
    let events=list_json(&mut tx,"SELECT to_jsonb(e) FROM singbox_commerce_events e WHERE user_id=$1 ORDER BY id DESC LIMIT 100",id).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"profile":profile,"history":history,"events":events}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Profile {
    name: String,
    email: Option<String>,
    state: String,
    notes: String,
    version: i64,
    reason: String,
}
pub(super) async fn member_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<Profile>,
) -> ApiResult<Json<Value>> {
    let actor = administrator(&state, &headers, true).await?;
    let name = crate::business::name(&input.name)?;
    let contact = input
        .email
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(email)
        .transpose()?;
    let notes = text(&input.notes, 4000, false)?;
    let reason = text(&input.reason, 500, true)?;
    if !["active", "pending", "suspended", "archived", "staged"].contains(&input.state.as_str()) {
        return Err(bad("成员状态无效"));
    }
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    ensure_member(&mut tx, id).await?;
    if let Some(email) = &contact {
        let duplicate: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM singbox_members WHERE email=$1 AND user_id<>$2)",
        )
        .bind(email)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if duplicate {
            return Err(conflict("联系邮箱已被其他成员使用"));
        }
    }
    let staged:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_nexus_identities i WHERE i.user_id=$1 AND NOT EXISTS(SELECT 1 FROM singbox_nexus_bridge b WHERE b.import_id=i.import_id AND b.enabled))").bind(id).fetch_one(&mut *tx).await?;
    if staged && ["active", "pending"].contains(&input.state.as_str()) {
        return Err(conflict(
            "待迁移成员须完成独立接管演练，不能通过编辑资料跳过迁移核对",
        ));
    }
    let changed=sqlx::query("UPDATE singbox_members SET email_verified_at=CASE WHEN email IS DISTINCT FROM $2 THEN NULL ELSE email_verified_at END,email=$2,state=$3,notes=$4,version=version+1 WHERE user_id=$1 AND version=$5").bind(id).bind(contact).bind(&input.state).bind(notes).bind(input.version).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(conflict("成员资料已变化，请刷新后再保存"));
    }
    sqlx::query("UPDATE users SET name=$2 WHERE id=$1")
        .bind(id)
        .bind(name)
        .execute(&mut *tx)
        .await?;
    if !["active", "pending"].contains(&input.state.as_str()) {
        revoke_on(&mut tx, id).await?;
    }
    let servers = crate::business::lock_user_servers(&mut tx, id).await?;
    crate::business::mark_dirty(&mut tx, &servers).await?;
    event(
        &mut tx,
        Some(id),
        &actor,
        "member_changed",
        &id.to_string(),
        json!({"state":input.state,"reason":reason}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
pub(super) async fn revoke_on(tx: &mut Tx<'_>, id: i64) -> ApiResult<()> {
    sqlx::query("DELETE FROM singbox_portal_sessions WHERE account_id IN(SELECT account_id FROM singbox_portal_accounts WHERE user_id=$1)").bind(id).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM passkey_ceremonies WHERE account_id IN(SELECT account_id FROM singbox_portal_accounts WHERE user_id=$1)").bind(id).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn revoke_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let actor = administrator(&state, &headers, true).await?;
    let mut tx = state.pool.begin().await?;
    ensure_member(&mut tx, id).await?;
    revoke_on(&mut tx, id).await?;
    event(
        &mut tx,
        Some(id),
        &actor,
        "sessions_revoked",
        &id.to_string(),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Bulk {
    members: Vec<VersionedMember>,
    state: String,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionedMember {
    id: i64,
    version: i64,
}
pub(super) async fn bulk(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut input): Json<Bulk>,
) -> ApiResult<Json<Value>> {
    let actor = administrator(&state, &headers, true).await?;
    let reason = text(&input.reason, 500, true)?;
    if input.members.is_empty()
        || input.members.len() > 200
        || !["active", "suspended", "archived"].contains(&input.state.as_str())
    {
        return Err(bad("请选择 1–200 个成员与有效状态"));
    }
    input.members.sort_by_key(|m| m.id);
    if input.members.windows(2).any(|p| p[0].id == p[1].id) {
        return Err(bad("成员编号重复"));
    }
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    for member in &input.members {
        crate::business::lock_user(&mut tx, member.id).await?;
        let row=sqlx::query("SELECT COALESCE(m.version,0) AS version,COALESCE(m.state,'active') AS state FROM users u LEFT JOIN singbox_members m ON m.user_id=u.id WHERE u.id=$1").bind(member.id).fetch_one(&mut *tx).await?;
        if row.try_get::<i64, _>("version")? != member.version {
            return Err(conflict("部分成员已变化，整个批次未执行，请刷新"));
        }
        let unactivated:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_nexus_identities i WHERE i.user_id=$1 AND NOT EXISTS(SELECT 1 FROM singbox_nexus_bridge b WHERE b.import_id=i.import_id AND b.enabled))").bind(member.id).fetch_one(&mut *tx).await?;
        if unactivated || row.try_get::<String, _>("state")? == "staged" {
            return Err(conflict("批量操作不能修改待迁移成员"));
        }
        ensure_member(&mut tx, member.id).await?;
        sqlx::query("UPDATE singbox_members SET state=$2,version=version+1 WHERE user_id=$1")
            .bind(member.id)
            .bind(&input.state)
            .execute(&mut *tx)
            .await?;
        if input.state != "active" {
            revoke_on(&mut tx, member.id).await?;
        }
        let servers = crate::business::lock_user_servers(&mut tx, member.id).await?;
        crate::business::mark_dirty(&mut tx, &servers).await?;
        event(
            &mut tx,
            Some(member.id),
            &actor,
            "member_bulk_changed",
            &member.id.to_string(),
            json!({"state":input.state,"reason":reason}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"updated":input.members.len()})))
}

async fn history(tx: &mut Tx<'_>, id: i64) -> ApiResult<Value> {
    Ok(json!({
        "ledger":list_json(tx,"SELECT to_jsonb(l)-'request_hash' FROM singbox_wallet_ledger l WHERE user_id=$1 ORDER BY id DESC LIMIT 100",id).await?,
        "orders":list_json(tx,"SELECT to_jsonb(o)-'request_hash' FROM singbox_commerce_orders o WHERE payer_id=$1 OR beneficiary_id=$1 ORDER BY created_at DESC,id LIMIT 100",id).await?,
        "invitations":list_json(tx,"SELECT to_jsonb(i)-'token_hash'-'request_hash'-'claim_hash' FROM singbox_commerce_invitations i WHERE issuer_id=$1 ORDER BY created_at DESC,id LIMIT 100",id).await?,
        "topups":list_json(tx,"SELECT to_jsonb(t)-'request_hash'-'merchant' FROM singbox_payment_topups t WHERE user_id=$1 ORDER BY created_at DESC,id LIMIT 100",id).await?,
        "legacy":list_json(tx,"SELECT jsonb_build_object('kind',kind,'source_id',source_id,'record',record) FROM singbox_nexus_records WHERE user_id=$1 ORDER BY kind,source_id LIMIT 100",id).await?
    }))
}
pub(super) async fn portal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let is_enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM singbox_commerce_settings WHERE singleton")
            .fetch_one(&state.pool)
            .await?;
    if !is_enabled {
        return Ok(Json(json!({"enabled":false})));
    }
    let mut tx = state.pool.begin().await?;
    let user = member_session(&mut tx, &state, &headers, account, false).await?;
    let profile:Value=sqlx::query_scalar("SELECT jsonb_build_object('user_id',m.user_id,'email',m.email,'email_verified',m.email_verified_at IS NOT NULL,'has_password',m.password_hash IS NOT NULL,'remind_expire',m.remind_expire,'remind_traffic',m.remind_traffic,'remind_invitation',m.remind_invitation,'state',m.state,'sponsor_required',m.sponsor_required AND m.first_purchase_at IS NULL,'balance_cents',w.balance_cents) FROM singbox_members m JOIN singbox_wallets w ON w.user_id=m.user_id WHERE m.user_id=$1").bind(user).fetch_one(&mut *tx).await?;
    let offers: Vec<catalog::Offer> =
        sqlx::query_as("SELECT * FROM singbox_commerce_offers WHERE enabled ORDER BY id")
            .fetch_all(&mut *tx)
            .await?;
    let mut content:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(c) FROM singbox_member_content c WHERE published ORDER BY updated_at DESC,id DESC LIMIT 500").fetch_all(&mut *tx).await?;
    let allowed: bool =
        sqlx::query_scalar("SELECT allowed FROM singbox_entitlements($1) WHERE user_id=$2")
            .bind(now_timestamp())
            .bind(user)
            .fetch_one(&mut *tx)
            .await?;
    let (token,base):(String,Option<String>)=sqlx::query_as("SELECT u.subscription_token,b.subscription_base FROM users u LEFT JOIN singbox_nexus_identities i ON i.user_id=u.id LEFT JOIN singbox_nexus_bridge b ON b.import_id=i.import_id AND b.enabled WHERE u.id=$1").bind(user).fetch_one(&mut *tx).await?;
    let url = base
        .map(|base| format!("{base}{token}"))
        .unwrap_or_else(|| format!("{}/sub/{token}?format=singbox", state.config.public_url));
    for entry in &mut content {
        if let Some(body) = entry["body"].as_str() {
            entry["body"] = json!(content::render(body, allowed, &url));
        }
    }
    let channels:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'name',name,'fixed_fee_cents',fixed_fee_cents,'fee_basis_points',fee_basis_points,'version',version) FROM singbox_payment_channels WHERE enabled ORDER BY id").fetch_all(&mut *tx).await?;
    let invite_settings:Value=sqlx::query_scalar("SELECT jsonb_build_object('enabled',invitations_enabled,'price_cents',invite_price_cents) FROM singbox_commerce_settings WHERE singleton").fetch_one(&mut *tx).await?;
    let friends=list_json(&mut tx,"SELECT jsonb_build_object('user_id',m.user_id,'name',u.name,'state',m.state,'awaiting_sponsor',m.sponsor_required AND m.first_purchase_at IS NULL) FROM singbox_members m JOIN users u ON u.id=m.user_id WHERE m.inviter_id=$1 AND u.deleted_at IS NULL ORDER BY m.user_id LIMIT 100",user).await?;
    let history = history(&mut tx, user).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"enabled":true,"profile":profile,"offers":offers,"content":content,"channels":channels,"invite_settings":invite_settings,"friends":friends,"history":history}),
    ))
}

//! Invitations by quota (ADR 0113). A proxy user spends one invitation from their
//! quota on each code; the person who redeems it creates their own account, which
//! starts with the package, policy groups and quota the administrator chose.
use super::*;
use crate::auth::hash_token;
use axum::{
    Json,
    extract::{Path, State},
};
use rand::Rng;

const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LENGTH: usize = 16;
const MAX_QUOTA: i32 = 100_000;

/// A new code: 16 characters of Crockford base32 (80 bits), shown in groups of four.
fn new_code() -> String {
    let mut rng = rand::thread_rng();
    (0..CODE_LENGTH)
        .map(|_| char::from(ALPHABET[rng.gen_range(0..ALPHABET.len())]))
        .collect()
}
fn display(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// The canonical form of what someone typed or followed: a code, read leniently, or
/// the 43-character token of a link issued while invitations were paid.
pub(super) fn normalize(input: &str) -> Option<String> {
    let input = input.trim();
    if input.len() == 43
        && input
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Some(input.to_owned());
    }
    let code: String = input
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect();
    (code.len() == CODE_LENGTH && code.bytes().all(|c| ALPHABET.contains(&c))).then_some(code)
}

fn link(state: &AppState, code: &str) -> String {
    format!(
        "{}/#/plugins/sing-box/join/{}",
        state.config.public_url,
        display(code)
    )
}

/// Whether a code can still create an account: invitations are open, the code is
/// unused and its issuer is still a usable account.
pub(super) async fn usable(tx: &mut Tx<'_>, hash: &str) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_invitations i JOIN users u ON u.id=i.issuer_id LEFT JOIN singbox_members m ON m.user_id=i.issuer_id WHERE i.token_hash=$1 AND i.state='available' AND u.deleted_at IS NULL AND COALESCE(m.state,'active') IN('active','pending') AND (SELECT enabled FROM singbox_invitation_settings WHERE singleton))")
        .bind(hash).fetch_one(&mut **tx).await?)
}

async fn quota_change(
    tx: &mut Tx<'_>,
    user: i64,
    delta: i32,
    reason: &str,
    actor: &str,
    detail: Value,
) -> ApiResult<i32> {
    sqlx::query("INSERT INTO singbox_invitation_quotas(user_id) VALUES($1) ON CONFLICT DO NOTHING")
        .bind(user)
        .execute(&mut **tx)
        .await?;
    let remaining: i32 = sqlx::query_scalar(
        "SELECT remaining FROM singbox_invitation_quotas WHERE user_id=$1 FOR UPDATE",
    )
    .bind(user)
    .fetch_one(&mut **tx)
    .await?;
    let next = remaining.saturating_add(delta);
    if next < 0 {
        return Err(conflict("邀请名额不足"));
    }
    if next > MAX_QUOTA {
        return Err(bad("邀请名额最多 100000 个"));
    }
    sqlx::query("UPDATE singbox_invitation_quotas SET remaining=$2 WHERE user_id=$1")
        .bind(user)
        .bind(next)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO singbox_invitation_quota_events(user_id,delta,remaining,reason,actor,detail,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(user).bind(delta).bind(next).bind(reason).bind(actor).bind(detail).bind(now_timestamp()).execute(&mut **tx).await?;
    Ok(next)
}

const ISSUED: &str = "SELECT jsonb_build_object('id',i.id,'hint',i.hint,'origin',i.origin,'state',i.state,'created_at',i.created_at,'claimed_at',i.claimed_at,'revoked_at',i.revoked_at,'recipient',CASE WHEN r.id IS NULL THEN NULL ELSE jsonb_build_object('id',r.id,'name',r.name) END) FROM singbox_invitations i LEFT JOIN users r ON r.id=i.recipient_id WHERE i.issuer_id=$1 ORDER BY i.created_at DESC,i.id LIMIT 200";

/// The portal's view of a user's own invitations.
pub(super) async fn summary(tx: &mut Tx<'_>, user: i64) -> ApiResult<Value> {
    let enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM singbox_invitation_settings WHERE singleton")
            .fetch_one(&mut **tx)
            .await?;
    let remaining: i32 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT remaining FROM singbox_invitation_quotas WHERE user_id=$1),0)",
    )
    .bind(user)
    .fetch_one(&mut **tx)
    .await?;
    let issued = list_json(tx, ISSUED, user).await?;
    Ok(json!({"enabled":enabled,"remaining":remaining,"issued":issued}))
}

#[derive(Serialize, sqlx::FromRow)]
pub(super) struct Settings {
    enabled: bool,
    package_group_id: Option<i64>,
    policy_group_ids: Vec<i64>,
    default_quota: i32,
    version: i64,
}

async fn load_settings(tx: &mut Tx<'_>) -> ApiResult<Settings> {
    Ok(sqlx::query_as("SELECT s.enabled,s.package_group_id,ARRAY(SELECT group_id FROM singbox_invitation_policy_groups ORDER BY group_id) AS policy_group_ids,s.default_quota,s.version FROM singbox_invitation_settings s WHERE singleton")
        .fetch_one(&mut **tx).await?)
}

pub(super) async fn settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Settings>> {
    administrator(&state, &headers, false).await?;
    let mut tx = state.pool.begin().await?;
    let value = load_settings(&mut tx).await?;
    tx.commit().await?;
    Ok(Json(value))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SettingsInput {
    enabled: bool,
    package_group_id: Option<i64>,
    policy_group_ids: Vec<i64>,
    default_quota: i32,
    version: i64,
}

pub(super) async fn settings_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<SettingsInput>,
) -> ApiResult<Json<Settings>> {
    let actor = administrator(&state, &headers, true).await?;
    if !(0..=1000).contains(&input.default_quota) {
        return Err(bad("受邀账号的初始邀请名额为 0 至 1000"));
    }
    let groups = crate::policies::ids(&input.policy_group_ids)?;
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM singbox_policy_groups WHERE id=ANY($1)")
            .bind(&groups)
            .fetch_one(&mut *tx)
            .await?;
    if count != groups.len() as i64 {
        return Err(bad("所选策略组已不存在，请刷新后重新选择"));
    }
    if let Some(package) = input.package_group_id {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_package_groups WHERE id=$1)")
                .bind(package)
                .fetch_one(&mut *tx)
                .await?;
        if !exists {
            return Err(bad("所选套餐已不存在，请刷新后重新选择"));
        }
        // Invited accounts are members of the compatibility bridge while it runs; an
        // unmapped package there would stop every later synchronization.
        let unmapped: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_nexus_bridge b WHERE b.enabled AND NOT EXISTS(SELECT 1 FROM singbox_nexus_plan_mappings p WHERE p.import_id=b.import_id AND p.package_group_id=$1 AND p.legacy_group_id IS NOT NULL))")
            .bind(package).fetch_one(&mut *tx).await?;
        if unmapped {
            return Err(conflict(
                "原订阅兼容期间，受邀账号只能使用已对应原权限组的套餐",
            ));
        }
    }
    let changed = sqlx::query("UPDATE singbox_invitation_settings SET enabled=$1,package_group_id=$2,default_quota=$3,version=version+1,updated_at=$4 WHERE singleton AND version=$5")
        .bind(input.enabled).bind(input.package_group_id).bind(input.default_quota).bind(now_timestamp()).bind(input.version)
        .execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(conflict("邀请设置已变化，请刷新后再保存"));
    }
    sqlx::query("DELETE FROM singbox_invitation_policy_groups")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO singbox_invitation_policy_groups(group_id) SELECT unnest($1::bigint[])",
    )
    .bind(&groups)
    .execute(&mut *tx)
    .await?;
    event(
        &mut tx,
        None,
        &actor,
        "invitation_settings_saved",
        "settings",
        json!({"enabled":input.enabled,"package_group_id":input.package_group_id,"policy_group_ids":groups,"default_quota":input.default_quota}),
    )
    .await?;
    let value = load_settings(&mut tx).await?;
    tx.commit().await?;
    Ok(Json(value))
}

/// An administrator's view of one user: quota, its history, codes and the accounts
/// they brought in.
pub(super) async fn user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    administrator(&state, &headers, false).await?;
    let mut tx = state.pool.begin().await?;
    let remaining: Option<i32> = sqlx::query_scalar("SELECT COALESCE((SELECT remaining FROM singbox_invitation_quotas WHERE user_id=u.id),0) FROM users u WHERE u.id=$1 AND u.deleted_at IS NULL")
        .bind(id).fetch_optional(&mut *tx).await?;
    let remaining = remaining.ok_or(ApiError::NotFound)?;
    let events = list_json(&mut tx, "SELECT to_jsonb(e)-'user_id' FROM singbox_invitation_quota_events e WHERE user_id=$1 ORDER BY id DESC LIMIT 50", id).await?;
    let issued = list_json(&mut tx, ISSUED, id).await?;
    let inviter: Option<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',u.id,'name',u.name) FROM singbox_members m JOIN users u ON u.id=m.inviter_id WHERE m.user_id=$1")
        .bind(id).fetch_optional(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"remaining":remaining,"events":events,"issued":issued,"invited_by":inviter}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Adjust {
    delta: i32,
    reason: String,
}

pub(super) async fn adjust(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<Adjust>,
) -> ApiResult<Json<Value>> {
    let actor = administrator(&state, &headers, true).await?;
    if input.delta == 0 || input.delta.abs() > MAX_QUOTA {
        return Err(bad("调整数量须为非零整数，且不超过 100000"));
    }
    let reason = text(&input.reason, 200, false)?;
    let mut tx = state.pool.begin().await?;
    crate::business::lock_user(&mut tx, id).await?;
    let remaining = quota_change(
        &mut tx,
        id,
        input.delta,
        "administrator",
        &actor,
        json!({"reason":reason}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"remaining":remaining})))
}

fn one() -> i32 {
    1
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Create {
    #[serde(default = "one")]
    quantity: i32,
    request_id: Uuid,
}

pub(super) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<Uuid>,
    Json(input): Json<Create>,
) -> ApiResult<Json<Value>> {
    if !(1..=20).contains(&input.quantity) {
        return Err(bad("一次可生成 1 至 20 个邀请码"));
    }
    let mut tx = state.pool.begin().await?;
    let user = account_session(&mut tx, &state, &headers, account, true).await?;
    crate::business::lock_user(&mut tx, user).await?;
    let enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM singbox_invitation_settings WHERE singleton")
            .fetch_one(&mut *tx)
            .await?;
    if !enabled {
        return Err(ApiError::Forbidden("管理员暂未开放邀请".into()));
    }
    let allowed: bool = sqlx::query_scalar(
        "SELECT COALESCE((SELECT allowed FROM singbox_entitlements($1) WHERE user_id=$2),FALSE)",
    )
    .bind(now_timestamp())
    .bind(user)
    .fetch_one(&mut *tx)
    .await?;
    if !allowed {
        return Err(ApiError::Forbidden("账号当前不可用，暂不能邀请他人".into()));
    }
    let repeated: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_invitations WHERE issuer_id=$1 AND request_id=$2)",
    )
    .bind(user)
    .bind(input.request_id)
    .fetch_one(&mut *tx)
    .await?;
    if repeated {
        return Ok(Json(
            json!({"codes":[],"links":[],"message":"这些邀请码已经生成；邀请码只显示一次，未保存的可在列表中重新生成。"}),
        ));
    }
    let remaining = quota_change(
        &mut tx,
        user,
        -input.quantity,
        "invitation_created",
        &format!("account:{user}"),
        json!({"quantity":input.quantity}),
    )
    .await?;
    let mut codes = Vec::new();
    let mut links = Vec::new();
    for index in 0..input.quantity {
        let code = new_code();
        sqlx::query("INSERT INTO singbox_invitations(id,issuer_id,token_hash,hint,request_id,created_at) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(Uuid::new_v4()).bind(user).bind(hash_token(&code)).bind(&code[CODE_LENGTH - 4..])
            .bind((index == 0).then_some(input.request_id)).bind(now_timestamp()).execute(&mut *tx).await?;
        links.push(link(&state, &code));
        codes.push(display(&code));
    }
    tx.commit().await?;
    Ok(Json(
        json!({"codes":codes,"links":links,"remaining":remaining}),
    ))
}

pub(super) async fn rotate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((account, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    let user = account_session(&mut tx, &state, &headers, account, true).await?;
    let code = new_code();
    let changed = sqlx::query("UPDATE singbox_invitations SET token_hash=$3,hint=$4 WHERE id=$1 AND issuer_id=$2 AND state='available'")
        .bind(id).bind(user).bind(hash_token(&code)).bind(&code[CODE_LENGTH - 4..]).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(conflict("邀请已使用或已撤销，不能重新生成"));
    }
    event(
        &mut tx,
        Some(user),
        &format!("account:{user}"),
        "invitation_rotated",
        &id.to_string(),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"code":display(&code),"link":link(&state,&code)}),
    ))
}

/// Withdraws an unused code; the invitation goes back to the issuer's quota.
pub(super) async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((account, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    let user = account_session(&mut tx, &state, &headers, account, true).await?;
    crate::business::lock_user(&mut tx, user).await?;
    let status: String = sqlx::query_scalar(
        "SELECT state FROM singbox_invitations WHERE id=$1 AND issuer_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(user)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    if status == "claimed" {
        return Err(conflict("邀请已被使用，不能撤销"));
    }
    let mut remaining = None;
    if status == "available" {
        sqlx::query("UPDATE singbox_invitations SET state='revoked',revoked_at=$2 WHERE id=$1")
            .bind(id)
            .bind(now_timestamp())
            .execute(&mut *tx)
            .await?;
        remaining = Some(
            quota_change(
                &mut tx,
                user,
                1,
                "invitation_revoked",
                &format!("account:{user}"),
                json!({"invitation_id":id}),
            )
            .await?,
        );
    }
    tx.commit().await?;
    Ok(Json(
        json!({"id":id,"state":"revoked","remaining":remaining}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Token {
    token: String,
}

pub(super) async fn inspect(
    State(state): State<AppState>,
    Json(input): Json<Token>,
) -> ApiResult<Json<Value>> {
    let code = normalize(&input.token).ok_or(ApiError::NotFound)?;
    let mut tx = state.pool.begin().await?;
    let available = usable(&mut tx, &hash_token(&code)).await?;
    let known: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_invitations WHERE token_hash=$1)")
            .bind(hash_token(&code))
            .fetch_one(&mut *tx)
            .await?;
    if !known {
        return Err(ApiError::NotFound);
    }
    let package: Option<String> = sqlx::query_scalar("SELECT g.name FROM singbox_invitation_settings s JOIN singbox_package_groups g ON g.id=s.package_group_id WHERE s.singleton")
        .fetch_optional(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"available":available,"package_name":package,"configuration":state.passkeys.info(),"email_verification_required":mail::config(&state).await?.enabled}),
    ))
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

pub(super) async fn claim(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Claim>,
) -> ApiResult<Json<Value>> {
    state.passkeys.check_origin(&headers)?;
    let token = normalize(&input.token).ok_or(ApiError::NotFound)?;
    let token_hash = hash_token(&token);
    let name = crate::business::name(&input.name)?;
    let email = email(&input.email)?;
    let hash = fingerprint(&(input.request_id, &name, &email))?;
    let verification_required = mail::config(&state).await?.enabled;
    let claimed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_invitations WHERE token_hash=$1 AND claim_hash=$2 AND state='claimed')").bind(&token_hash).bind(&hash).fetch_one(&state.pool).await?;
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
        && (challenge.address != email || challenge.invitation_hash.as_deref() != Some(&token_hash))
    {
        return Err(bad("邮箱验证码与本邀请不匹配"));
    }
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let row = sqlx::query("SELECT * FROM singbox_invitations WHERE token_hash=$1 FOR UPDATE")
        .bind(&token_hash)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    let id: Uuid = row.try_get("id")?;
    let issuer: i64 = row.try_get("issuer_id")?;
    let status: String = row.try_get("state")?;
    let (user, account) = if status == "claimed" {
        if row.try_get::<Option<String>, _>("claim_hash")?.as_deref() != Some(&hash) {
            return Err(conflict("邀请已被使用"));
        }
        let user: i64 = row.try_get("recipient_id")?;
        let account: Uuid =
            sqlx::query_scalar("SELECT account_id FROM singbox_portal_accounts WHERE user_id=$1")
                .bind(user)
                .fetch_one(&mut *tx)
                .await?;
        (user, account)
    } else {
        if status != "available" || !usable(&mut tx, &token_hash).await? {
            return Err(conflict("邀请已撤销或暂不可用"));
        }
        if verification_required && challenge.is_none() {
            return Err(bad("请先验证联系邮箱"));
        }
        if let Some(challenge) = &challenge {
            accounts::consume(&mut tx, challenge).await?;
        }
        let duplicate: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_members WHERE email=$1)")
                .bind(&email)
                .fetch_one(&mut *tx)
                .await?;
        if duplicate {
            return Err(conflict("此联系邮箱已登记，请使用原账号登录或联系管理员"));
        }
        let user: i64 = sqlx::query_scalar(
            "INSERT INTO users(name,subscription_token) VALUES($1,$2) RETURNING id",
        )
        .bind(name)
        .bind(crate::auth::random_token())
        .fetch_one(&mut *tx)
        .await?;
        create_profile(&mut tx, user, Some(&email), Some(issuer)).await?;
        if challenge.is_some() {
            sqlx::query("UPDATE singbox_members SET email_verified_at=$2 WHERE user_id=$1")
                .bind(user)
                .bind(now_timestamp())
                .execute(&mut *tx)
                .await?;
        }
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
        let (package, quota): (Option<i64>, i32) = sqlx::query_as(
            "SELECT package_group_id,default_quota FROM singbox_invitation_settings WHERE singleton",
        )
        .fetch_one(&mut *tx)
        .await?;
        if let Some(package) = package {
            crate::packages::assign_on(&mut tx, user, package, id).await?;
        }
        sqlx::query("INSERT INTO singbox_user_policies(user_id,group_id) SELECT $1,group_id FROM singbox_invitation_policy_groups")
            .bind(user)
            .execute(&mut *tx)
            .await?;
        crate::policies::sync_users(&mut tx, &[user]).await?;
        if quota > 0 {
            quota_change(
                &mut tx,
                user,
                quota,
                "invited_account",
                "system",
                json!({"invitation_id":id}),
            )
            .await?;
        }
        sqlx::query("UPDATE singbox_invitations SET state='claimed',recipient_id=$2,claimed_at=$3,claim_hash=$4 WHERE id=$1")
            .bind(id).bind(user).bind(now_timestamp()).bind(&hash).execute(&mut *tx).await?;
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
        let token = crate::auth::random_token();
        sqlx::query("UPDATE singbox_portal_accounts SET activation_hash=$2,activation_expires_at=$3 WHERE user_id=$1").bind(user).bind(hash_token(&token)).bind(now_timestamp()+900).execute(&mut *tx).await?;
        url.push_str(&format!("?activate={token}"));
    }
    tx.commit().await?;
    Ok(Json(json!({"url":url})))
}

#[cfg(test)]
mod tests {
    use super::{display, new_code, normalize};

    #[test]
    fn codes_are_read_leniently_and_old_links_still_resolve() {
        let code = new_code();
        assert_eq!(normalize(&display(&code)).as_deref(), Some(code.as_str()));
        assert_eq!(
            normalize(&display(&code).to_lowercase()).as_deref(),
            Some(code.as_str())
        );
        assert_eq!(
            normalize("0IL1-ABCD-EFGH-JKMN").as_deref(),
            Some("0111ABCDEFGHJKMN")
        );
        assert_eq!(
            normalize("oooo-aaaa-bbbb-cccc").as_deref(),
            Some("0000AAAABBBBCCCC")
        );
        let legacy = "a".repeat(43);
        assert_eq!(normalize(&legacy).as_deref(), Some(legacy.as_str()));
        assert!(normalize("ABCD-EFGH").is_none());
        assert!(normalize("UUUU-AAAA-BBBB-CCCC").is_none());
    }
}

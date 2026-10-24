use super::*;
use axum::{
    Json,
    extract::{Path, State},
};

pub(super) async fn add(
    tx: &mut Tx<'_>,
    user: i64,
    key: &str,
    kind: &str,
    message: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO singbox_member_messages(user_id,event_key,kind,message,created_at,mail_state,next_attempt_at) SELECT $1,$2,$3,$4,$5,CASE WHEN s.enabled AND s.reminders_enabled AND m.email_verified_at IS NOT NULL AND CASE $3 WHEN 'sponsor' THEN m.remind_invitation WHEN 'expiry' THEN m.remind_expire WHEN 'traffic' THEN m.remind_traffic ELSE FALSE END THEN 'pending' ELSE 'disabled' END,$5 FROM singbox_members m CROSS JOIN singbox_mail_settings s WHERE m.user_id=$1 AND s.singleton ON CONFLICT(event_key) DO NOTHING").bind(user).bind(key).bind(kind).bind(message).bind(now_timestamp()).execute(&mut **tx).await?;
    Ok(())
}
pub(super) async fn activation(tx: &mut Tx<'_>, recipient: i64, inviter: i64) -> ApiResult<()> {
    add(
        tx,
        inviter,
        &format!("sponsor:{recipient}:{}", now_timestamp() / 86400),
        "sponsor",
        &format!("你邀请的成员 #{recipient} 正在等待首次开通，可在邀请的朋友中选择套餐代购。"),
    )
    .await
}
pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    let id = member_session(&mut tx, &state, &headers, account, false).await?;
    let entries=list_json(&mut tx,"SELECT jsonb_build_object('id',id,'kind',kind,'message',message,'created_at',created_at,'read_at',read_at) FROM singbox_member_messages WHERE user_id=$1 ORDER BY id DESC LIMIT 200",id).await?;
    tx.commit().await?;
    Ok(Json(json!({"entries":entries})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Read {
    ids: Vec<i64>,
}
pub(super) async fn read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<Uuid>,
    Json(input): Json<Read>,
) -> ApiResult<Json<Value>> {
    if input.ids.len() > 200 {
        return Err(bad("单次最多标记 200 条消息"));
    }
    let mut tx = state.pool.begin().await?;
    let id = member_session(&mut tx, &state, &headers, account, true).await?;
    sqlx::query("UPDATE singbox_member_messages SET read_at=COALESCE(read_at,$3) WHERE user_id=$1 AND id=ANY($2)").bind(id).bind(input.ids).bind(now_timestamp()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preferences {
    remind_expire: bool,
    remind_traffic: bool,
    remind_invitation: bool,
}
pub(super) async fn preferences(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<Uuid>,
    Json(input): Json<Preferences>,
) -> ApiResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    let id = member_session(&mut tx, &state, &headers, account, true).await?;
    sqlx::query("UPDATE singbox_members SET remind_expire=$2,remind_traffic=$3,remind_invitation=$4,version=version+1 WHERE user_id=$1").bind(id).bind(input.remind_expire).bind(input.remind_traffic).bind(input.remind_invitation).execute(&mut *tx).await?;
    sqlx::query("UPDATE singbox_member_messages SET mail_state='cancelled' WHERE user_id=$1 AND mail_state IN('pending','failed') AND ((kind='expiry' AND NOT $2) OR (kind='traffic' AND NOT $3) OR (kind='sponsor' AND NOT $4))").bind(id).bind(input.remind_expire).bind(input.remind_traffic).bind(input.remind_invitation).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
pub(super) async fn remind(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let id = member_session(&mut tx, &state, &headers, account, true).await?;
    let inviter:Option<i64>=sqlx::query_scalar("SELECT inviter_id FROM singbox_members WHERE user_id=$1 AND sponsor_required AND first_purchase_at IS NULL").bind(id).fetch_optional(&mut *tx).await?.flatten();
    let inviter = inviter.ok_or_else(|| conflict("当前账户无需邀请人开通"))?;
    ensure_member(&mut tx, inviter).await?;
    active(&mut tx, inviter).await?;
    let last:Option<i64>=sqlx::query_scalar("SELECT max(created_at) FROM singbox_member_messages WHERE user_id=$1 AND event_key LIKE $2").bind(inviter).bind(format!("sponsor:{id}:%")).fetch_one(&mut *tx).await?;
    if last.is_none_or(|time| time <= now_timestamp() - 86400) {
        activation(&mut tx, id, inviter).await?;
    }
    tx.commit().await?;
    Ok(Json(
        json!({"message":"已提醒邀请人；每 24 小时最多提醒一次。","next_remind_at":last.filter(|t|*t>now_timestamp()-86400).unwrap_or(now_timestamp())+86400}),
    ))
}
pub(super) async fn run(state: AppState) {
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        timer.tick().await;
        if let Err(error) = tick(&state).await {
            tracing::warn!(%error,"member notification maintenance failed");
        }
    }
}
async fn tick(state: &AppState) -> ApiResult<()> {
    let mut tx = state.pool.begin().await?;
    crate::entitlements::lock(&mut tx).await?;
    let enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM singbox_commerce_settings WHERE singleton")
            .fetch_one(&mut *tx)
            .await?;
    if !enabled {
        return Ok(());
    }
    let now = now_timestamp();
    let rows=sqlx::query("SELECT e.user_id,e.expires_at,e.next_reset,e.used_bytes,e.monthly_bytes,m.remind_expire,m.remind_traffic FROM singbox_entitlements($1) e JOIN singbox_members m ON m.user_id=e.user_id WHERE m.state='active' AND ((m.remind_expire AND e.expires_at BETWEEN $1 AND $1+86400) OR (m.remind_traffic AND e.monthly_bytes IS NOT NULL AND e.used_bytes::numeric>=e.monthly_bytes::numeric*0.8)) ORDER BY e.user_id LIMIT 10000").bind(now).fetch_all(&mut *tx).await?;
    for row in rows {
        let user: i64 = row.try_get("user_id")?;
        if row.try_get::<bool, _>("remind_expire")?
            && let Some(expires) = row.try_get::<Option<i64>, _>("expires_at")?
            && expires <= now + 86400
            && expires >= now
        {
            add(
                &mut tx,
                user,
                &format!("expiry:{user}:{expires}"),
                "expiry",
                "套餐将在 24 小时内到期，请按需要续费。",
            )
            .await?;
        }
        if row.try_get::<bool, _>("remind_traffic")? {
            let used = row
                .try_get::<String, _>("used_bytes")?
                .parse::<u128>()
                .unwrap_or(0);
            let quota = row
                .try_get::<Option<String>, _>("monthly_bytes")?
                .and_then(|v| v.parse::<u128>().ok());
            if quota.is_some_and(|q| q > 0 && used * 5 >= q * 4) {
                let cycle = row.try_get::<Option<i64>, _>("next_reset")?.unwrap_or(0);
                add(
                    &mut tx,
                    user,
                    &format!("traffic:{user}:{cycle}"),
                    "traffic",
                    "本期受管流量已达到套餐额度的 80%，请查看用量与重置日期。",
                )
                .await?;
            }
        }
    }
    sqlx::query("DELETE FROM singbox_account_challenges WHERE expires_at<$1")
        .bind(now - 86400)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM singbox_account_limits WHERE starts_at<$1")
        .bind(now - 86400)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let config = mail::config(state).await?;
    if !config.enabled || !config.reminders_enabled {
        return Ok(());
    }
    for _ in 0..20 {
        let mut tx = state.pool.begin().await?;
        let row=sqlx::query("SELECT n.id,n.message,m.email FROM singbox_member_messages n JOIN singbox_members m ON m.user_id=n.user_id JOIN users u ON u.id=m.user_id WHERE m.state IN('active','pending') AND u.deleted_at IS NULL AND m.email_verified_at IS NOT NULL AND n.mail_attempts<5 AND ((n.mail_state IN('pending','failed') AND n.next_attempt_at<=$1) OR (n.mail_state='sending' AND n.lease_until<$1)) AND CASE n.kind WHEN 'sponsor' THEN m.remind_invitation WHEN 'expiry' THEN m.remind_expire WHEN 'traffic' THEN m.remind_traffic ELSE FALSE END ORDER BY n.id LIMIT 1 FOR UPDATE OF n SKIP LOCKED").bind(now_timestamp()).fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            break;
        };
        let id: i64 = row.try_get("id")?;
        sqlx::query("UPDATE singbox_member_messages SET mail_state='sending',mail_attempts=mail_attempts+1,lease_until=$2 WHERE id=$1").bind(id).bind(now_timestamp()+120).execute(&mut *tx).await?;
        tx.commit().await?;
        let message: String = row.try_get("message")?;
        let email: String = row.try_get("email")?;
        let result = mail::send(state, &config, &email, "apsp 成员服务提醒", &message).await;
        sqlx::query("UPDATE singbox_member_messages SET mail_state=$2,next_attempt_at=$3,lease_until=NULL WHERE id=$1 AND mail_state='sending'").bind(id).bind(if result.is_ok(){"sent"}else{"failed"}).bind(now_timestamp()+300).execute(&state.pool).await?;
    }
    Ok(())
}

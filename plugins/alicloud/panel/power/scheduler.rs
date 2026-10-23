use super::super::{
    account_on, billing,
    client::Cloud,
    lock,
    model::{Account, Resource},
    notices, resource,
};
use super::{
    State,
    jobs::{self, Intent},
};
use crate::error::ApiResult;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub(in super::super) async fn poll(pool: &PgPool, id: Uuid, cloud: &Cloud) -> ApiResult<()> {
    let initial = resource(pool, id).await?;
    let mut tx = lock(pool, initial.account_id).await?;
    let account = account_on(&mut tx, initial.account_id).await?;
    let mut r = jobs::fresh(&mut tx, id).await?;
    let now = apsp_protocol::now_timestamp();
    if r.kind != "ecs" || !account.enabled || r.next_power_at > now {
        return Ok(());
    }
    sqlx::query("UPDATE alicloud_resources SET next_power_at=$2 WHERE id=$1")
        .bind(id)
        .bind(now + 60)
        .execute(&mut *tx)
        .await?;
    match cloud.power_state(&account, &r).await {
        Ok(state) => {
            jobs::state_on(&mut tx, id, &state, now).await?;
            evaluate(&mut tx, &account, &mut r, &state, now).await?;
        }
        Err(error) => {
            sqlx::query(
                "UPDATE alicloud_resources SET power_error=$2,next_power_at=$3 WHERE id=$1",
            )
            .bind(id)
            .bind(error.code)
            .bind(now + error.retry_after.max(60))
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}
pub(super) async fn evaluate(
    tx: &mut Transaction<'_, Postgres>,
    account: &Account,
    r: &mut Resource,
    state: &State,
    now: i64,
) -> ApiResult<()> {
    let p = r.power_policy.0.clone();
    if !p.enabled {
        return Ok(());
    }
    let exceeded = p.exceeded(account, now);
    if p.threshold_action == "stop" {
        if let Some(value) = exceeded {
            r.threshold_hold = value;
            sqlx::query("UPDATE alicloud_resources SET threshold_hold=$2 WHERE id=$1")
                .bind(r.id)
                .bind(value)
                .execute(&mut **tx)
                .await?;
        }
        if r.threshold_hold || exceeded.is_none() {
            // Cancel a queued automatic start before it can defeat new protection.
            sqlx::query("UPDATE alicloud_power_jobs SET status='cancelled',error_code='policy_inactive',updated_at=$2 WHERE resource_id=$1 AND action='start' AND status='queued'").bind(r.id).bind(now).execute(&mut **tx).await?;
        }
    }
    if p.threshold_action != "off" && exceeded == Some(true) {
        notices::record(
            tx,
            r,
            &format!("threshold:{}:{}:{}", r.id, r.revision, billing::month(now)),
            "CDT 流量阈值提醒",
            &format!(
                "账号当月已出账 CDT 流量达到 {} GB 额度的 {}%。{}",
                p.limit_gb,
                p.threshold_percent,
                if p.threshold_action == "stop" {
                    "已启用停机保护，执行结果请查看任务记录。"
                } else {
                    "当前策略仅通知。"
                }
            ),
            now,
        )
        .await?;
    }
    let intent = if p.threshold_action == "stop"
        && exceeded == Some(true)
        && state.status == "Running"
    {
        Some((
            "stop",
            "threshold",
            format!("threshold:{}:{}:{}", r.id, r.revision, now / 900),
            now + 300,
        ))
    } else if p.schedule_enabled && p.occurrence("stop", now).is_some() && !p.in_window(now) {
        let occurrence = p.occurrence("stop", now).unwrap();
        Some((
            "stop",
            "schedule",
            format!("schedule:{}:stop:{occurrence}", r.id),
            occurrence + 600,
        ))
    } else if !r.manual_hold
        && !p.blocks_start(account, r, now)
        && p.schedule_enabled
        && p.in_window(now)
        && p.occurrence("start", now).is_some()
    {
        let occurrence = p.occurrence("start", now).unwrap();
        Some((
            "start",
            "schedule",
            format!("schedule:{}:start:{occurrence}", r.id),
            occurrence + 600,
        ))
    } else if p.keepalive
        && r.next_power_at <= now
        && !r.manual_hold
        && !p.blocks_start(account, r, now)
        && state.spot()
        && state.status == "Stopped"
        && (!p.schedule_enabled || p.in_window(now))
    {
        let recent: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM alicloud_power_jobs WHERE resource_id=$1 AND action='start' AND status<>'preview' AND GREATEST(created_at,updated_at)>$2)").bind(r.id).bind(now-900).fetch_one(&mut **tx).await?;
        if recent {
            None
        } else {
            Some((
                "start",
                "keepalive",
                format!("keepalive:{}:{}", r.id, now / 900),
                now + 300,
            ))
        }
    } else {
        None
    };
    if let Some((action, source, key, expires_at)) = intent {
        if source == "threshold" {
            let recent: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM alicloud_power_jobs WHERE resource_id=$1 AND source='threshold' AND created_at>$2)")
                .bind(r.id).bind(now-900).fetch_one(&mut **tx).await?;
            if recent {
                return Ok(());
            }
        }
        if state.validate(action, &p.stop_mode).is_err() {
            return Ok(());
        }
        // A bandwidth operation and a power operation cannot run on one resource together.
        if let Err(error) = super::super::operations::idle(tx, r.id).await {
            return match error {
                crate::error::ApiError::Conflict(_) => Ok(()),
                other => Err(other),
            };
        }
        jobs::prepare(
            tx,
            account,
            r,
            state,
            Intent {
                action,
                mode: &p.stop_mode,
                source,
                key: Some(key),
                expires_at,
            },
            now,
        )
        .await?;
    }
    Ok(())
}

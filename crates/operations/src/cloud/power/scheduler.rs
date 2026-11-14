use super::jobs::{self, Intent};
use super::model::State;
use crate::cloud::automation::{self, Automated};
use crate::error::ApiResult;
use apsp_costs::cloud::{
    accounts::{self, AccountBilling},
    billing,
};
use apsp_observability::cloud_notices;
use apsp_resources::cloud::{account_on, addresses, client::Cloud, lock, model::Account, resource};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub async fn poll(pool: &PgPool, id: Uuid, cloud: &Cloud) -> ApiResult<()> {
    let initial = resource(pool, id).await?;
    let mut tx = lock(pool, initial.account_id).await?;
    let account = account_on(&mut tx, initial.account_id).await?;
    let billing = accounts::account_on(&mut tx, initial.account_id).await?;
    let mut r = automation::current(&mut tx, id).await?;
    let now = apsp_protocol::now_timestamp();
    if r.kind != "ecs" || !account.enabled || r.automation.next_power_at > now {
        return Ok(());
    }
    // Without a policy the poll only refreshes the displayed state, which the
    // five-minute account refresh also reads. Saving a policy resets this time.
    let interval = if r.automation.power_policy.enabled {
        60
    } else {
        300
    };
    automation::row(&mut tx, id).await?;
    sqlx::query("UPDATE cloud_resource_automation SET next_power_at=$2 WHERE resource_id=$1")
        .bind(id)
        .bind(now + interval)
        .execute(&mut *tx)
        .await?;
    match crate::cloud::power::state::power_state(cloud, &account, &r).await {
        Ok(state) => {
            jobs::state_on(&mut tx, id, &state, now).await?;
            // Address supplementation is optional and does not change state validity.
            let reading = cloud.ipv6(&account, &r, &state.public_ipv6).await;
            addresses::ipv6_on(&mut tx, id, &reading).await?;
            if r.automation.power_policy.cdt_guard.is_some() {
                super::guard::poll(&mut tx, &account, &billing, &mut r, &state, cloud, now).await?;
            } else {
                evaluate(&mut tx, &account, &billing, &mut r, &state, now).await?;
            }
        }
        Err(error) => {
            sqlx::query(
                "UPDATE cloud_resource_automation SET power_error=$2,next_power_at=$3 WHERE resource_id=$1",
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
    billing: &AccountBilling,
    r: &mut Automated,
    state: &State,
    now: i64,
) -> ApiResult<()> {
    let p = r.automation.power_policy.0.clone();
    if !p.enabled {
        return Ok(());
    }
    let guarded = p.cdt_guard.is_some();
    let exceeded = if guarded {
        super::guard::exceeded(account, r, now)
    } else {
        p.exceeded(account, billing, now)
    };
    if guarded && exceeded != Some(false) {
        sqlx::query("UPDATE alicloud_power_jobs SET status='cancelled',error_code='policy_inactive',updated_at=$2 WHERE resource_id=$1 AND action='start' AND status='queued'").bind(r.id).bind(now).execute(&mut **tx).await?;
    }
    if p.threshold_action == "stop" {
        if let Some(value) = exceeded {
            r.automation.threshold_hold = value;
            automation::row(tx, r.id).await?;
            sqlx::query(
                "UPDATE cloud_resource_automation SET threshold_hold=$2 WHERE resource_id=$1",
            )
            .bind(r.id)
            .bind(value)
            .execute(&mut **tx)
            .await?;
        }
        if r.automation.threshold_hold || exceeded.is_none() {
            // Cancel a queued automatic start before it can defeat new protection.
            sqlx::query("UPDATE alicloud_power_jobs SET status='cancelled',error_code='policy_inactive',updated_at=$2 WHERE resource_id=$1 AND action='start' AND status='queued'").bind(r.id).bind(now).execute(&mut **tx).await?;
        }
    }
    if p.threshold_action != "off" && exceeded == Some(true) {
        cloud_notices::record(
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
    let intent = if guarded && exceeded == Some(true) && state.status == "Running" {
        Some((
            "stop",
            "cdt_guard",
            format!("cdt-guard:{}:{}:{}", r.id, r.revision, now / 60),
            now + 90,
        ))
    } else if p.threshold_action == "stop" && exceeded == Some(true) && state.status == "Running" {
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
    } else if !r.automation.manual_hold
        && !p.blocks_start(account, billing, r, now)
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
    } else if guarded
        && exceeded == Some(false)
        && state.status == "Stopped"
        && !r.automation.manual_hold
        && p.cdt_guard.as_ref().is_some_and(|g| g.auto_start)
        && (!p.schedule_enabled || p.in_window(now))
    {
        Some((
            "start",
            "cdt_guard",
            format!("cdt-guard:{}:{}:{}", r.id, r.revision, now / 60),
            now + 90,
        ))
    } else if p.keepalive
        && r.automation.next_power_at <= now
        && !r.automation.manual_hold
        && !p.blocks_start(account, billing, r, now)
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
        if let Err(error) = crate::cloud::bandwidth::idle(tx, r.id).await {
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

use super::super::{
    account_on,
    client::Cloud,
    lock,
    model::{Account, Resource},
    notices, resource,
};
use super::{Job, State, scheduler};
use crate::error::{ApiError, ApiResult};
use sqlx::{PgPool, Postgres, Transaction, types::Json};
use uuid::Uuid;

pub(super) async fn load(pool: &PgPool, id: Uuid) -> ApiResult<Job> {
    sqlx::query_as("SELECT * FROM alicloud_power_jobs WHERE id=$1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(ApiError::NotFound)
}
pub(super) async fn fresh(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<Resource> {
    sqlx::query_as("SELECT * FROM alicloud_resources WHERE id=$1 AND NOT archived")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)
}
pub(in super::super) async fn idle(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<()> {
    let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM alicloud_power_jobs WHERE resource_id=$1 AND status IN ('queued','running','uncertain'))").bind(id).fetch_one(&mut **tx).await?;
    if busy {
        return Err(ApiError::Conflict(
            "此资源有待处理的启停任务，请先核对结果".into(),
        ));
    }
    Ok(())
}
pub(super) async fn state_on(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    state: &State,
    now: i64,
) -> ApiResult<()> {
    sqlx::query("UPDATE alicloud_resources SET power_state=$2,power_checked_at=$3,power_error=NULL WHERE id=$1").bind(id).bind(Json(state)).bind(now).execute(&mut **tx).await?;
    Ok(())
}
pub(super) struct Intent<'a> {
    pub action: &'a str,
    pub mode: &'a str,
    pub source: &'a str,
    pub key: Option<String>,
    pub expires_at: i64,
}
pub(super) async fn prepare(
    tx: &mut Transaction<'_, Postgres>,
    account: &Account,
    resource: &Resource,
    state: &State,
    intent: Intent<'_>,
    now: i64,
) -> ApiResult<Option<Uuid>> {
    super::super::operations::idle(tx, resource.id).await?;
    let id = Uuid::new_v4();
    let result = sqlx::query("INSERT INTO alicloud_power_jobs(id,resource_id,account_revision,resource_revision,action,stop_mode,source,dedup_key,before_state,status,created_at,expires_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$11) ON CONFLICT(dedup_key) DO NOTHING")
        .bind(id).bind(resource.id).bind(account.revision).bind(resource.revision).bind(intent.action).bind(intent.mode).bind(intent.source).bind(intent.key).bind(Json(state)).bind(if intent.source=="manual"{"preview"}else{"queued"}).bind(now).bind(intent.expires_at).execute(&mut **tx).await?;
    Ok((result.rows_affected() != 0).then_some(id))
}
pub(super) fn allowed(job: &Job, account: &Account, resource: &Resource, now: i64) -> bool {
    if !account.enabled
        || resource.kind != "ecs"
        || account.revision != job.account_revision
        || resource.revision != job.resource_revision
        || job.expires_at <= now
    {
        return false;
    }
    let policy = &resource.power_policy;
    if job.action == "start" && policy.blocks_start(account, resource, now) {
        return false;
    }
    if job.source == "manual" {
        return true;
    }
    if !policy.enabled || (job.action == "start" && resource.manual_hold) {
        return false;
    }
    if policy.cdt_guard.is_some() {
        return job.source == "cdt_guard"
            && match job.action.as_str() {
                "stop" => super::guard::exceeded(account, resource, now) == Some(true),
                "start" => {
                    policy.cdt_guard.as_ref().is_some_and(|g| g.auto_start)
                        && super::guard::exceeded(account, resource, now) == Some(false)
                }
                _ => false,
            };
    }
    match job.source.as_str() {
        "threshold" => {
            policy.threshold_action == "stop" && policy.exceeded(account, now) == Some(true)
        }
        "schedule" => {
            policy.schedule_enabled
                && policy.occurrence(&job.action, now).is_some()
                && (job.action == "start") == policy.in_window(now)
        }
        "keepalive" => {
            policy.keepalive
                && job.before_state.spot()
                && (!policy.schedule_enabled || policy.in_window(now))
        }
        _ => false,
    }
}
async fn finish(
    tx: &mut Transaction<'_, Postgres>,
    job: &Job,
    resource: &Resource,
    status: &str,
    error: Option<&str>,
    now: i64,
) -> ApiResult<()> {
    sqlx::query("UPDATE alicloud_power_jobs SET status=$2,error_code=$3,updated_at=$4,next_check_at=$5 WHERE id=$1")
        .bind(job.id).bind(status).bind(error).bind(now).bind(now+60).execute(&mut **tx).await?;
    if matches!(status, "succeeded" | "failed" | "uncertain") {
        let verb = if job.action == "start" {
            "开机"
        } else {
            "停机"
        };
        let result = match status {
            "succeeded" => "已核对完成",
            "failed" => "未按预期完成",
            _ => "结果待核对",
        };
        let detail = error.map(super::super::model::message).unwrap_or("");
        notices::record(
            tx,
            resource,
            &format!("power:{}:{status}", job.id),
            "ECS 启停结果",
            &format!("{verb}{result}。{detail}"),
            now,
        )
        .await?;
    }
    Ok(())
}
async fn readback(
    tx: &mut Transaction<'_, Postgres>,
    job: &Job,
    account: &Account,
    resource: &Resource,
    cloud: &Cloud,
    now: i64,
) -> ApiResult<()> {
    match cloud.power_state(account, resource).await {
        Ok(state) => {
            state_on(tx, resource.id, &state, now).await?;
            let target = if job.action == "start" {
                "Running"
            } else {
                "Stopped"
            };
            let (status, error) = if state.status == target {
                if job.action == "stop" && state.charge_type == "PostPaid" {
                    match &state.stopped_mode {
                        Some(mode) if mode != &job.stop_mode => {
                            ("failed", Some("stop_mode_mismatch"))
                        }
                        None => ("uncertain", Some("stop_mode_unknown")),
                        _ => ("succeeded", None),
                    }
                } else {
                    ("succeeded", None)
                }
            } else {
                ("uncertain", Some("awaiting_confirmation"))
            };
            finish(tx, job, resource, status, error, now).await?;
        }
        Err(error) => {
            finish(tx, job, resource, "uncertain", Some(error.code), now).await?;
            defer(tx, job.id, resource.id, now + error.retry_after.max(60)).await?;
        }
    }
    Ok(())
}
async fn defer(
    tx: &mut Transaction<'_, Postgres>,
    job: Uuid,
    resource: Uuid,
    until: i64,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE alicloud_power_jobs SET next_check_at=GREATEST(next_check_at,$2) WHERE id=$1",
    )
    .bind(job)
    .bind(until)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE alicloud_resources SET next_power_at=GREATEST(next_power_at,$2) WHERE id=$1",
    )
    .bind(resource)
    .bind(until)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
pub(in super::super) async fn process(pool: &PgPool, id: Uuid, cloud: &Cloud) -> ApiResult<()> {
    let initial = load(pool, id).await?;
    let r = resource(pool, initial.resource_id).await?;
    let mut tx = lock(pool, r.account_id).await?;
    let job: Job = sqlx::query_as("SELECT * FROM alicloud_power_jobs WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let account = account_on(&mut tx, r.account_id).await?;
    let r = fresh(&mut tx, r.id).await?;
    let now = apsp_protocol::now_timestamp();
    if job.next_check_at > now {
        return Ok(());
    }
    if matches!(job.status.as_str(), "running" | "uncertain") {
        readback(&mut tx, &job, &account, &r, cloud, now).await?;
        tx.commit().await?;
        return Ok(());
    }
    if job.status != "queued" {
        return Ok(());
    }
    if !allowed(&job, &account, &r, now) {
        finish(&mut tx, &job, &r, "cancelled", Some("policy_inactive"), now).await?;
        tx.commit().await?;
        return Ok(());
    }
    let current = match cloud.power_state(&account, &r).await {
        Ok(value) => value,
        Err(error) => {
            finish(&mut tx, &job, &r, "failed", Some(error.code), now).await?;
            defer(&mut tx, id, r.id, now + error.retry_after.max(60)).await?;
            tx.commit().await?;
            return Ok(());
        }
    };
    state_on(&mut tx, r.id, &current, now).await?;
    if current.status
        == if job.action == "start" {
            "Running"
        } else {
            "Stopped"
        }
    {
        // A new command never cycles power just to change an already stopped mode.
        let mismatch = job.action == "stop"
            && current.charge_type == "PostPaid"
            && current.stopped_mode.as_deref() != Some(&job.stop_mode);
        finish(
            &mut tx,
            &job,
            &r,
            if mismatch { "failed" } else { "succeeded" },
            mismatch.then_some("stop_mode_mismatch"),
            now,
        )
        .await?;
        tx.commit().await?;
        return Ok(());
    }
    if current != job.before_state.0 || (job.source == "keepalive" && !current.spot()) {
        finish(&mut tx, &job, &r, "failed", Some("state_changed"), now).await?;
        tx.commit().await?;
        return Ok(());
    }
    if let Err(error) = current.validate(&job.action, &job.stop_mode) {
        finish(&mut tx, &job, &r, "failed", Some(error.code), now).await?;
        tx.commit().await?;
        return Ok(());
    }
    sqlx::query("UPDATE alicloud_power_jobs SET status='running',updated_at=$2,next_check_at=$3 WHERE id=$1").bind(id).bind(now).bind(now+60).execute(&mut *tx).await?;
    tx.commit().await?;
    // The durable intent precedes the write. Recovery never resends this job.
    let mut tx = lock(pool, r.account_id).await?;
    let account = account_on(&mut tx, r.account_id).await?;
    let r = fresh(&mut tx, r.id).await?;
    let job: Job = sqlx::query_as("SELECT * FROM alicloud_power_jobs WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if job.status != "running" {
        return Ok(());
    }
    let now = apsp_protocol::now_timestamp();
    if !allowed(&job, &account, &r, now) {
        finish(&mut tx, &job, &r, "cancelled", Some("policy_inactive"), now).await?;
    } else {
        match cloud
            .power_control(&account, &r, &job.action, &job.stop_mode)
            .await
        {
            Ok(request_id) => {
                sqlx::query("UPDATE alicloud_power_jobs SET request_id=$2 WHERE id=$1")
                    .bind(id)
                    .bind(request_id)
                    .execute(&mut *tx)
                    .await?;
                readback(
                    &mut tx,
                    &job,
                    &account,
                    &r,
                    cloud,
                    apsp_protocol::now_timestamp(),
                )
                .await?;
            }
            Err(error) => {
                let observed_at = apsp_protocol::now_timestamp();
                let rejected = matches!(
                    error.code,
                    "capacity_unavailable"
                        | "insufficient_balance"
                        | "resource_locked"
                        | "resource_not_found"
                        | "request_rejected"
                );
                finish(
                    &mut tx,
                    &job,
                    &r,
                    if rejected { "failed" } else { "uncertain" },
                    Some(error.code),
                    observed_at,
                )
                .await?;
                let cooldown = if job.source == "keepalive" && rejected {
                    900
                } else {
                    60
                };
                defer(
                    &mut tx,
                    id,
                    r.id,
                    observed_at + error.retry_after.max(cooldown),
                )
                .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(())
}
pub(in super::super) async fn tick(pool: &PgPool, cloud: &Cloud) -> ApiResult<()> {
    let now = apsp_protocol::now_timestamp();
    let jobs: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM alicloud_power_jobs WHERE status IN ('queued','running','uncertain') AND next_check_at<=$1 ORDER BY next_check_at,created_at LIMIT 8").bind(now).fetch_all(pool).await?;
    for id in jobs {
        process(pool, id, cloud).await?;
    }
    let resources: Vec<Uuid> = sqlx::query_scalar("SELECT r.id FROM alicloud_resources r JOIN alicloud_accounts a ON a.id=r.account_id WHERE r.kind='ecs' AND NOT r.archived AND NOT a.archived AND a.enabled AND r.next_power_at<=$1 ORDER BY r.next_power_at,r.id LIMIT 4").bind(now).fetch_all(pool).await?;
    for id in resources {
        scheduler::poll(pool, id, cloud).await?;
    }
    sqlx::query("UPDATE alicloud_power_jobs SET status='cancelled',updated_at=$1 WHERE status IN ('preview','queued') AND expires_at<=$1").bind(now).execute(pool).await?;
    sqlx::query("DELETE FROM alicloud_power_jobs WHERE status NOT IN ('queued','running','uncertain') AND created_at<$1").bind(now-180*86400).execute(pool).await?;
    Ok(())
}

use super::{
    api::history,
    model::{Plan, digest},
};
use crate::{
    AppState, control_center,
    error::{ApiError, ApiResult},
};
use apsp_protocol::now_timestamp;
use apsp_traffic::runtime_workflows::automation;
use axum::http::HeaderMap;
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

pub(super) async fn backup_snapshot(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> ApiResult<Value> {
    sqlx::query_scalar("SELECT jsonb_build_object('id',id,'name',name,'recipient',recipient,'interval_secs',interval_secs,'retention_count',retention_count,'retention_days',retention_days,'paused',paused) FROM operations_backup_schedules WHERE id=$1")
        .bind(id).fetch_optional(&mut **tx).await?.ok_or(ApiError::NotFound)
}

pub(super) async fn preview(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    targets: &[i64],
    plan: &Plan,
) -> ApiResult<Value> {
    let mut previews = Vec::new();
    for (position, step) in plan.steps.iter().enumerate() {
        if step.is_panel() {
            let backup = backup_snapshot(
                tx,
                step.backup_schedule_id
                    .ok_or_else(|| ApiError::BadRequest("备份计划缺失".into()))?,
            )
            .await?;
            previews.push(json!({"position":position,"kind":step.kind,"backup_digest":digest(&backup)?,"backup":backup,"once_per_job":true,"phase":"首台部署前的一次恢复点"}));
        } else if step.is_runtime() {
            let mut candidates = serde_json::Map::new();
            for server in targets {
                candidates.insert(
                    server.to_string(),
                    automation::automation_candidate(state, tx, *server).await?,
                );
            }
            previews.push(json!({"position":position,"kind":step.kind,"targets":candidates,"scope":"仅重新应用当前明确失败的固定配置修订，不生成新的业务配置或选择独立运行时制品"}));
        }
    }
    Ok(json!(previews))
}

pub(super) async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    plan: &Plan,
    read: bool,
) -> ApiResult<()> {
    if plan.steps.iter().any(|step| step.is_panel()) {
        crate::control_center::require_global_material(
            state,
            headers,
            if read {
                "recovery:read"
            } else {
                "recovery:write"
            },
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn actor_authorized(state: &AppState, actor: i64, plan: &Plan) -> ApiResult<bool> {
    if !plan.steps.iter().any(|step| step.is_panel()) {
        return Ok(true);
    }
    let global:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM administrator_profiles WHERE admin_id=$1 AND enabled AND all_servers)")
        .bind(actor).fetch_one(&state.pool).await?;
    if !global {
        return Ok(false);
    }
    match control_center::require_actor_capability(state, actor, "recovery:write").await {
        Ok(()) => Ok(true),
        Err(ApiError::Forbidden(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

fn snapshot(spec: &Value, position: usize) -> ApiResult<&Value> {
    spec["typed_steps"]
        .as_array()
        .and_then(|values| {
            values
                .iter()
                .find(|value| value["position"].as_u64() == Some(position as u64))
        })
        .ok_or_else(|| ApiError::Conflict("固定执行器预览缺失，请重新建立并确认方案".into()))
}

fn is_legacy_candidate(candidate: &Value) -> bool {
    candidate.get("artifact").is_some()
        || candidate.get("runtime_version").is_some()
        || candidate.get("agent_bundle").is_none()
}

fn has_legacy_runtime_preview(spec: &Value) -> bool {
    spec["typed_steps"].as_array().is_some_and(|steps| {
        steps.iter().any(|step| {
            step["kind"].as_str() == Some("singbox_retry_deployment")
                && (step.get("version").is_some()
                    || step["targets"]
                        .as_object()
                        .is_none_or(|targets| targets.values().any(is_legacy_candidate)))
        })
    })
}

pub(super) async fn materialize(
    tx: &mut Transaction<'_, Postgres>,
    job: Uuid,
    plan: &Plan,
    spec: &Value,
) -> ApiResult<()> {
    for (position, step) in plan.steps.iter().enumerate() {
        if step.is_panel() {
            let frozen = snapshot(spec, position)?;
            sqlx::query("INSERT INTO operations_panel_steps(job_id,position,execution_id,spec) VALUES($1,$2,$3,$4)")
                .bind(job).bind(position as i32).bind(Uuid::new_v4()).bind(frozen).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

pub(super) async fn validate_frozen(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    targets: &[i64],
    plan: &Plan,
    spec: &Value,
) -> ApiResult<()> {
    if plan.has_legacy_runtime_version() || has_legacy_runtime_preview(spec) {
        return Err(ApiError::Conflict(
            "旧版独立制品部署预览已失效，请移除运行时版本参数并重新预览确认".into(),
        ));
    }
    plan.validate(targets)?;
    let current = preview(state, tx, targets, plan).await?;
    if current
        != spec
            .get("typed_steps")
            .cloned()
            .unwrap_or_else(|| json!([]))
    {
        return Err(ApiError::Conflict(
            "备份设置、固定失败配置或可信 Agent 整包身份已变化，请重新预览；不会悄悄选择新配置"
                .into(),
        ));
    }
    Ok(())
}

pub(super) struct RuntimeStep<'a> {
    pub job: Uuid,
    pub server: i64,
    pub position: i32,
    pub actor: i64,
    pub spec: &'a Value,
    pub expires: i64,
}

pub(super) async fn enqueue_runtime(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    step: RuntimeStep<'_>,
) -> ApiResult<()> {
    let RuntimeStep {
        job,
        server,
        position,
        actor,
        spec,
        expires,
    } = step;
    if has_legacy_runtime_preview(spec) {
        return Err(ApiError::Conflict(
            "旧版独立制品部署候选已失效，请重新预览并确认方案".into(),
        ));
    }
    let frozen = snapshot(spec, position as usize)?;
    if frozen.get("version").is_some() {
        return Err(ApiError::Conflict(
            "旧版独立制品部署候选已失效，请重新预览并确认方案".into(),
        ));
    }
    let candidate = frozen["targets"]
        .get(server.to_string())
        .ok_or_else(|| ApiError::Conflict("服务器的固定失败部署候选缺失".into()))?;
    if is_legacy_candidate(candidate) {
        return Err(ApiError::Conflict(
            "旧版独立制品部署候选已失效，请重新预览并确认方案".into(),
        ));
    }
    let request = super::deployment_automation::enqueue_automation_deployment_tx(
        state, tx, server, job, actor, candidate, expires,
    )
    .await?;
    sqlx::query("UPDATE operations_target_steps SET state='queued',runtime_operation_id=$4 WHERE job_id=$1 AND server_id=$2 AND position=$3")
        .bind(job).bind(server).bind(position).bind(request).execute(&mut **tx).await?;
    history(tx,job,None,"step_dispatched",json!({"server_id":server,"position":position,"runtime_operation_id":request,"executor":"实际Agent固定失败部署重新应用与配置checkpoint","retry_after_unknown":false}),now_timestamp()).await
}

pub(super) async fn sync(tx: &mut Transaction<'_, Postgres>, job: Uuid, now: i64) -> ApiResult<()> {
    let runtime=sqlx::query("SELECT server_id,position,runtime_operation_id FROM operations_target_steps WHERE job_id=$1 AND runtime_operation_id IS NOT NULL AND state IN ('queued','running','cancel_requested','uncertain')")
        .bind(job).fetch_all(&mut **tx).await?;
    for row in runtime {
        let request: Uuid = row.get("runtime_operation_id");
        let receipt =
            super::deployment_automation::automation_deployment_receipt(tx, request).await?;
        let state = receipt["state"].as_str().unwrap_or("uncertain");
        let terminal = matches!(state, "succeeded" | "failed" | "cancelled");
        sqlx::query("UPDATE operations_target_steps SET state=$4,result=$5,started_at=COALESCE(started_at,$6),finished_at=CASE WHEN $7 THEN COALESCE(finished_at,$8) ELSE NULL END WHERE job_id=$1 AND server_id=$2 AND position=$3")
            .bind(job).bind(row.get::<i64,_>("server_id")).bind(row.get::<i32,_>("position")).bind(state).bind(&receipt)
            .bind(receipt["dispatched_at"].as_i64()).bind(terminal).bind(now).execute(&mut **tx).await?;
    }
    sqlx::query("UPDATE operations_target_steps t SET state=p.state,result=p.result,started_at=p.claimed_at,finished_at=p.finished_at FROM operations_panel_steps p WHERE p.job_id=$1 AND t.job_id=p.job_id AND t.position=p.position AND p.state<>'pending'")
        .bind(job).execute(&mut **tx).await?;
    let stopping: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM operations_target_steps WHERE job_id=$1 AND state IN ('failed','cancelled','uncertain'))",
    )
    .bind(job)
    .fetch_one(&mut **tx)
    .await?;
    if !stopping
        && let Some((status, spec)) = sqlx::query_as::<_, (String, Value)>(
            "SELECT status,spec FROM operations_jobs WHERE id=$1",
        )
        .bind(job)
        .fetch_optional(&mut **tx)
        .await?
        && matches!(status.as_str(), "queued" | "running" | "paused")
        && let Ok(plan) = serde_json::from_value::<Plan>(spec["plan"].clone())
        && (plan.has_legacy_runtime_version() || has_legacy_runtime_preview(&spec))
    {
        for (position, step) in plan.steps.iter().enumerate() {
            if step.is_runtime() {
                sqlx::query("UPDATE operations_target_steps SET state='failed',finished_at=$3,result=$4 WHERE job_id=$1 AND position=$2 AND state='pending'")
                    .bind(job)
                    .bind(position as i32)
                    .bind(now)
                    .bind(json!({"error":"旧版独立制品部署候选已失效，请重新预览并确认方案","executed":false}))
                    .execute(&mut **tx)
                    .await?;
            }
        }
    }
    Ok(())
}

pub(super) async fn cancel(
    tx: &mut Transaction<'_, Postgres>,
    job: Uuid,
    now: i64,
) -> ApiResult<()> {
    let ids:Vec<Uuid>=sqlx::query_scalar("SELECT runtime_operation_id FROM operations_target_steps WHERE job_id=$1 AND runtime_operation_id IS NOT NULL AND state IN ('queued','running','cancel_requested','uncertain')")
        .bind(job).fetch_all(&mut **tx).await?;
    for id in ids {
        super::deployment_automation::cancel_automation_deployment_tx(tx, id).await?;
    }
    sqlx::query("UPDATE operations_panel_steps SET state='cancelled',finished_at=$2,result=jsonb_build_object('executed',false,'reason','执行前已取消') WHERE job_id=$1 AND state IN ('pending','queued')")
        .bind(job).bind(now).execute(&mut **tx).await?;
    sync(tx, job, now).await
}

pub(super) async fn queue_backup(
    tx: &mut Transaction<'_, Postgres>,
    job: Uuid,
    now: i64,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE operations_panel_steps SET state='queued' WHERE job_id=$1 AND state='pending'",
    )
    .bind(job)
    .execute(&mut **tx)
    .await?;
    sync(tx, job, now).await?;
    history(
        tx,
        job,
        None,
        "panel_backup_queued",
        json!({"once_per_job":true,"scope":"整个面板的真实完整恢复点；所有目标共同依赖首步"}),
        now,
    )
    .await
}

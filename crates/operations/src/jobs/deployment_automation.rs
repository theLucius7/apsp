//! The runtime deployment step of jobs: a confirmed job retries a fixed failed deployment
//! as a runtime operation, follows its receipt until a device checkpoint confirms it, and
//! lets an administrator reconcile a result that stayed unknown.
//!
//! Operations decides when the step runs and records the attempt (its runtime operation and
//! the job history, which also keeps the candidate a request was bound to and the
//! checkpoints asked for it). Whether the step is legal, what it targets and what the device
//! confirmed stay with the traffic service (`apsp_traffic::runtime_workflows::automation`),
//! which these functions call.

mod reconciliation;
pub use reconciliation::{
    reconcile_automation_deployment_tx, request_automation_deployment_checkpoint_tx,
};

use crate::{
    AppState, control_center,
    error::{ApiError, ApiResult},
};
use apsp_protocol::{
    RuntimeBinding, RuntimeOperationError, RuntimeOperationRequest, RuntimeOperationResult,
    now_timestamp,
};
use apsp_traffic::runtime_workflows::automation::{self, RetryConfirmation};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

fn conflict(message: &str) -> ApiError {
    ApiError::Conflict(message.into())
}

/// The candidate the job bound to a runtime request when it queued the request.
async fn bound_candidate(
    tx: &mut Transaction<'_, Postgres>,
    job: Uuid,
    request: Uuid,
) -> ApiResult<Option<Value>> {
    Ok(sqlx::query_scalar("SELECT details->'candidate' FROM operations_history WHERE job_id=$1 AND action='runtime_deployment_bound' AND details->>'request_id'=$2 ORDER BY id LIMIT 1")
        .bind(job).bind(request.to_string()).fetch_optional(&mut **tx).await?)
}

/// Whether an earlier runtime operation on the server was interrupted and no verified
/// checkpoint has been taken since; its effect is unknown, so no retry goes on top of it.
async fn interrupted(tx: &mut Transaction<'_, Postgres>, server: i64) -> ApiResult<bool> {
    let finished: Vec<Option<i64>> = sqlx::query_scalar("SELECT (result->>'finished_at')::bigint FROM runtime_operations WHERE server_id=$1 AND module='singbox' AND reconciled_at IS NULL AND result->>'error'='interrupted'")
        .bind(server).fetch_all(&mut **tx).await?;
    for since in finished {
        if !automation::checkpoint_verified_since(tx, server, since).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn enqueue_automation_deployment_tx(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    job: Uuid,
    actor: i64,
    candidate: &Value,
    expires: i64,
) -> ApiResult<Uuid> {
    for capability in ["operations:write", "proxy:write"] {
        control_center::require_actor_server(state, actor, server, capability).await?;
    }
    crate::servers::lock(tx, server).await?;
    let now = now_timestamp();
    let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM operations_server_locks l JOIN operations_jobs j ON j.id=l.job_id WHERE l.server_id=$1 AND l.job_id=$2 AND j.requested_by=$3 AND j.status IN ('queued','running') AND j.cancel_requested_at IS NULL AND j.expires_at>$4 AND $1=ANY(j.targets))")
        .bind(server).bind(job).bind(actor).bind(now).fetch_one(&mut **tx).await?;
    if !owned {
        return Err(conflict("自动化任务未持有此服务器的有效互斥锁"));
    }
    if crate::runtime_operations::automation_blocked(tx, server, now).await? {
        return Err(conflict("服务器维护、退役或已有未核对操作，不能重试部署"));
    }
    if interrupted(tx, server).await? {
        return Err(conflict(
            "此前运行时操作曾中断且尚无之后的真实配置核对，不能自动重试未知动作",
        ));
    }
    let current = automation::automation_candidate(state, tx, server).await?;
    if &current != candidate {
        return Err(conflict(
            "预览中的失败部署、固定身份或签名制品已变化，请重新预览",
        ));
    }
    let request = automation::retry_request(candidate, now, expires)?;
    sqlx::query("INSERT INTO runtime_operations(id,server_id,module,requested_at,spec,automation_job_id) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(request.id).bind(server).bind(&request.module).bind(now).bind(serde_json::to_value(&request).map_err(anyhow::Error::from)?).bind(job).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO operations_history(job_id,actor,action,details,recorded_at) VALUES($1,$2,'runtime_deployment_bound',$3,$4)")
        .bind(job).bind(actor).bind(json!({"request_id":request.id,"server_id":server,"candidate":candidate,"replay":false})).bind(now).execute(&mut **tx).await?;
    Ok(request.id)
}

pub async fn cancel_automation_deployment_tx(
    tx: &mut Transaction<'_, Postgres>,
    request: Uuid,
) -> ApiResult<()> {
    sqlx::query("UPDATE runtime_operations SET cancelled_at=COALESCE(cancelled_at,$2) WHERE id=$1 AND automation_job_id IS NOT NULL AND result IS NULL AND dispatched_at IS NULL")
        .bind(request).bind(now_timestamp()).execute(&mut **tx).await?;
    Ok(())
}

pub async fn automation_dispatch_matches_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    request: Uuid,
) -> ApiResult<bool> {
    if !automation::retry_target_ready(tx, server).await? {
        return Ok(false);
    }
    let candidate: Option<Value> = sqlx::query_scalar("SELECT h.details->'candidate' FROM operations_history h JOIN runtime_operations r ON r.automation_job_id=h.job_id WHERE r.id=$1 AND r.server_id=$2 AND h.action='runtime_deployment_bound' AND h.details->>'request_id'=$1::text ORDER BY h.id LIMIT 1")
        .bind(request).bind(server).fetch_optional(&mut **tx).await?;
    let Some(candidate) = candidate else {
        return Ok(false);
    };
    automation::candidate_is_current(tx, server, &candidate).await
}

/// The checkpoint asked for a retried binding, recorded once per request in the job history.
async fn checkpoint_once(
    tx: &mut Transaction<'_, Postgres>,
    job: Uuid,
    request: Uuid,
    server: i64,
    binding: &RuntimeBinding,
    now: i64,
    dispatched_at: i64,
) -> ApiResult<Option<Uuid>> {
    let existing: Option<Value> = sqlx::query_scalar("SELECT details FROM operations_history WHERE job_id=$1 AND action='runtime_deployment_checkpoint' AND details->>'request_id'=$2 ORDER BY id LIMIT 1")
        .bind(job).bind(request.to_string()).fetch_optional(&mut **tx).await?;
    if let Some(existing) = existing {
        return Ok(existing["checkpoint_request_id"]
            .as_str()
            .and_then(|id| id.parse().ok()));
    }
    let Some(checkpoint) =
        automation::retry_checkpoint(tx, server, binding, now, dispatched_at).await?
    else {
        return Ok(None);
    };
    sqlx::query("INSERT INTO operations_history(job_id,action,details,recorded_at) VALUES($1,'runtime_deployment_checkpoint',$2,$3)")
        .bind(job).bind(json!({"request_id":request,"checkpoint_request_id":checkpoint,"binding":binding})).bind(now).execute(&mut **tx).await?;
    Ok(Some(checkpoint))
}

pub async fn automation_deployment_receipt(
    tx: &mut Transaction<'_, Postgres>,
    request_id: Uuid,
) -> ApiResult<Value> {
    // Keep the server-before-operation order used by Agent dispatch and completion.
    let server: i64 = sqlx::query_scalar("SELECT server_id FROM runtime_operations WHERE id=$1")
        .bind(request_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    crate::servers::lock(tx, server).await?;
    let row = sqlx::query("SELECT spec,result,dispatched_at,cancelled_at,automation_job_id,reconciled_at,reconciliation FROM runtime_operations WHERE id=$1 FOR UPDATE").bind(request_id).fetch_one(&mut **tx).await?;
    let job: Uuid = row
        .get::<Option<Uuid>, _>("automation_job_id")
        .ok_or_else(|| conflict("此运行时请求不属于自动化任务"))?;
    let request: RuntimeOperationRequest =
        serde_json::from_value(row.get("spec")).map_err(anyhow::Error::from)?;
    let dispatched: Option<i64> = row.get("dispatched_at");
    let result: Option<RuntimeOperationResult> = row
        .get::<Option<Value>, _>("result")
        .map(serde_json::from_value)
        .transpose()
        .map_err(anyhow::Error::from)?;
    if let Some(reconciled_at) = row.get::<Option<i64>, _>("reconciled_at") {
        return Ok(
            json!({"state":"failed","original_state":"uncertain","result":result,"request_id":request_id,"dispatched_at":dispatched,"finished_at":reconciled_at,"reconciled_at":reconciled_at,"reconciliation":row.get::<Option<Value>, _>("reconciliation"),"running_cancel_supported":false,"retry_unknown_result":false,"original_mutation_replayed":false}),
        );
    }
    let now = now_timestamp();
    if dispatched.is_none() && result.is_none() && request.expires_at <= now {
        cancel_automation_deployment_tx(tx, request_id).await?;
    }
    let cancelled = row.get::<Option<i64>, _>("cancelled_at").is_some()
        || (dispatched.is_none() && result.is_none() && request.expires_at <= now);
    let mut state = if cancelled {
        "cancelled"
    } else if dispatched.is_some() {
        if request.expires_at <= now {
            "uncertain"
        } else {
            "running"
        }
    } else {
        "queued"
    };
    let mut confirmed = false;
    let mut checkpoint_request_id: Option<Uuid> = None;
    if let Some(result) = &result {
        if !result.valid()
            || result.id != request_id
            || result.module != request.module
            || result.operation != request.operation
            || result.finished_at < request.requested_at
            || dispatched.is_none()
            || result.error == Some(RuntimeOperationError::Interrupted)
        {
            state = "uncertain";
        } else if result.error.is_some()
            || !result.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.healthy == Some(true)
                    && snapshot.applied_revision == request.expected_revision
                    && dispatched.is_some_and(|sent| snapshot.observed_at >= sent)
            })
        {
            state = "failed";
        } else if let Some(candidate) = bound_candidate(tx, job, request_id).await? {
            let dispatched_at = dispatched.unwrap_or(i64::MAX);
            match automation::retry_confirmation(
                tx,
                server,
                &candidate,
                &request,
                dispatched_at,
                now,
            )
            .await?
            {
                RetryConfirmation::Confirmed => {
                    confirmed = true;
                    state = "succeeded";
                }
                RetryConfirmation::Unconfirmed(binding) => {
                    checkpoint_request_id =
                        checkpoint_once(tx, job, request_id, server, &binding, now, dispatched_at)
                            .await?;
                    state = if request.expires_at <= now {
                        "uncertain"
                    } else {
                        "running"
                    };
                }
                RetryConfirmation::Mismatch => state = "uncertain",
            }
        } else {
            state = "uncertain";
        }
    }
    Ok(
        json!({"state":state,"result":result,"request_id":request_id,"dispatched_at":dispatched,"finished_at":result.as_ref().map(|result|result.finished_at),"expires_at":request.expires_at,"checkpoint_confirmed":confirmed,"checkpoint_request_id":checkpoint_request_id,"running_cancel_supported":false,"local_cancelled_before_dispatch":cancelled,"retry_unknown_result":false}),
    )
}

#[cfg(test)]
mod tests;

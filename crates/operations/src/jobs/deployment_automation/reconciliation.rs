//! Reconciling a dispatched retry whose result stayed unknown: a read-only checkpoint of the
//! fixed deployment, then an administrator's record once the device verified it. The
//! original result is kept and the original mutation is never replayed.

use super::*;
use apsp_protocol::RuntimeCheckpointRequest;
use apsp_traffic::runtime_workflows::automation::FixedDeployment;

struct UnknownDeployment {
    server: i64,
    job: Uuid,
    deployment: FixedDeployment,
    cutoff: i64,
    original_result: Option<Value>,
}

async fn unknown_deployment(
    tx: &mut Transaction<'_, Postgres>,
    original: Uuid,
) -> ApiResult<UnknownDeployment> {
    let server: i64 = sqlx::query_scalar("SELECT server_id FROM runtime_operations WHERE id=$1")
        .bind(original)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    crate::servers::lock(tx, server).await?;
    let row = sqlx::query("SELECT automation_job_id,spec,result,dispatched_at,reconciled_at FROM runtime_operations WHERE id=$1 FOR UPDATE").bind(original).fetch_one(&mut **tx).await?;
    let job: Uuid = row
        .get::<Option<Uuid>, _>("automation_job_id")
        .ok_or_else(|| conflict("此请求不属于自动化部署"))?;
    if row.get::<Option<i64>, _>("reconciled_at").is_some() {
        return Err(conflict("此未知部署已记录人工核对，请刷新结果"));
    }
    let dispatched: i64 = row
        .get::<Option<i64>, _>("dispatched_at")
        .ok_or_else(|| conflict("未派发部署应直接取消，无需未知结果核对"))?;
    let request: RuntimeOperationRequest =
        serde_json::from_value(row.get("spec")).map_err(anyhow::Error::from)?;
    let raw: Option<Value> = row.get("result");
    let result: Option<RuntimeOperationResult> = raw
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .map_err(anyhow::Error::from)?;
    let cutoff = if let Some(result) = result {
        if result.error != Some(RuntimeOperationError::Interrupted) {
            return Err(conflict(
                "此请求已有明确设备结果，请等待实际部署核对，不应人工改写结果",
            ));
        }
        dispatched.max(result.finished_at)
    } else {
        if request.expires_at > now_timestamp() {
            return Err(conflict("原请求尚在允许执行窗口内，先等待实际回执"));
        }
        dispatched.max(request.expires_at)
    };
    let saved = bound_candidate(tx, job, original)
        .await?
        .ok_or_else(|| conflict("原部署的固定候选事实缺失，不能宣称已核对"))?;
    let deployment = automation::unknown_retry_target(tx, server, &saved, &request).await?;
    Ok(UnknownDeployment {
        server,
        job,
        deployment,
        cutoff,
        original_result: raw,
    })
}

pub async fn request_automation_deployment_checkpoint_tx(
    tx: &mut Transaction<'_, Postgres>,
    original: Uuid,
) -> ApiResult<RuntimeCheckpointRequest> {
    let unknown = unknown_deployment(tx, original).await?;
    let now = now_timestamp();
    if now < unknown.cutoff {
        return Err(conflict("原执行时间尚未进入核对窗口，请等待时钟差异消除"));
    }
    let checkpoint =
        automation::request_unknown_checkpoint(tx, unknown.server, unknown.deployment.binding, now)
            .await?;
    sqlx::query("INSERT INTO operations_history(job_id,action,details,recorded_at) VALUES($1,'runtime_unknown_checkpoint',$2,$3)").bind(unknown.job).bind(json!({"request_id":original,"checkpoint_request_id":checkpoint.request_id,"read_only":true,"original_mutation_replayed":false})).bind(now).execute(&mut **tx).await?;
    Ok(checkpoint)
}

pub async fn reconcile_automation_deployment_tx(
    tx: &mut Transaction<'_, Postgres>,
    original: Uuid,
    checkpoint: Uuid,
    evidence: &str,
) -> ApiResult<Value> {
    if !crate::text::Text::bytes(4096)
        .not_blank()
        .multiline()
        .accepts(evidence)
    {
        return Err(ApiError::BadRequest(
            "人工核对说明须为 1–4096 字节的普通文字".into(),
        ));
    }
    let unknown = unknown_deployment(tx, original).await?;
    let now = now_timestamp();
    let requested: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM operations_history WHERE job_id=$1 AND action='runtime_unknown_checkpoint' AND details->>'request_id'=$2 AND details->>'checkpoint_request_id'=$3::text)")
        .bind(unknown.job).bind(original.to_string()).bind(checkpoint).fetch_one(&mut **tx).await?;
    let verified = requested
        && automation::unknown_checkpoint_verified(
            tx,
            unknown.server,
            checkpoint,
            unknown.cutoff,
            &unknown.deployment.binding,
            now,
        )
        .await?;
    if !verified {
        return Err(conflict(
            "尚无本次只读核对的新鲜 verified 回执，或原部署、当前实例和目标不匹配",
        ));
    }
    let reconciliation = json!({"checkpoint_request_id":checkpoint,"binding":unknown.deployment.binding,"artifact_sha256":unknown.deployment.artifact_sha256,"verified_at":now,"evidence":evidence,"original_state":"uncertain","original_result":unknown.original_result,"original_mutation_replayed":false,"original_result_rewritten":false,"checkpoint_scope":"Agent共同运行时门锁内核对签名实例和配置，且确认没有未完成运行时意图"});
    sqlx::query("UPDATE runtime_operations SET reconciled_at=$2,reconciliation=$3 WHERE id=$1 AND reconciled_at IS NULL").bind(original).bind(now).bind(&reconciliation).execute(&mut **tx).await?;
    Ok(reconciliation)
}

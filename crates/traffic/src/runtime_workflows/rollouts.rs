//! Staged rollouts of the proxy runtime's trusted version. The runtime upgrade belongs to the
//! traffic service and its execution to operations: the rollout, its members and its rules
//! live here (which version and targets are allowed, whether a target is ready for its
//! batch, whether a member confirmed the version), while operations dispatches the batches
//! with a read-only runtime inspection per target and hands the inspection results back.

use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use apsp_protocol::{RuntimeOperationResult, now_timestamp};
use axum::http::HeaderMap;
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

/// A rollout locked for dispatching its next batch.
pub struct Rollout {
    /// Every target in dispatch order, the canary first.
    pub targets: Vec<i64>,
    /// How many targets each batch after the canary takes.
    pub batch_size: i32,
    /// How many targets were dispatched so far.
    pub dispatched_count: i32,
    pub paused: bool,
    /// The signed artifact the rollout chose for each target when it was created.
    artifact_snapshot: Value,
}

/// A member whose inspection result has not reached the rollout.
pub struct PendingInspection {
    rollout: Uuid,
    server: i64,
    /// The member's read-only runtime inspection.
    pub request_id: Uuid,
}

/// The targets in dispatch order: the canary first, then the others by id.
pub fn targets(server_ids: &[i64], canary: Option<i64>) -> ApiResult<Vec<i64>> {
    let mut targets = crate::business::ids(server_ids, false)?;
    if let Some(canary) = canary {
        let position = targets
            .iter()
            .position(|id| *id == canary)
            .ok_or_else(|| ApiError::BadRequest("首台验证服务器必须属于固定目标集合".into()))?;
        targets.remove(position);
        targets.insert(0, canary);
    }
    Ok(targets)
}

/// A rollout installs the fixed trusted runtime, present in the signed inventory, in batches
/// of 1–20 after the canary.
pub async fn require_supported(state: &AppState, version: &str, batch_size: i32) -> ApiResult<()> {
    if version != crate::RUNTIME_VERSION || !(1..=20).contains(&batch_size) {
        return Err(ApiError::BadRequest(format!(
            "当前仅支持固定可信版本 {}，批次大小 1–20",
            crate::RUNTIME_VERSION
        )));
    }
    super::inventory::require_version(state, version).await
}

/// The unfinished rollouts that share a target with `targets`.
pub async fn unfinished_tx(
    tx: &mut Transaction<'_, Postgres>,
    targets: &[i64],
) -> ApiResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar("SELECT id FROM singbox_runtime_rollouts WHERE targets&&$1::bigint[] AND completed_at IS NULL").bind(targets).fetch_all(&mut **tx).await?)
}

/// Records a rollout with the signed artifact each target's platform selects now; the
/// rollout keeps these candidates and never replaces them.
pub async fn create_tx(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    administrator: i64,
    version: &str,
    targets: &[i64],
    batch_size: i32,
) -> ApiResult<Uuid> {
    let mut artifacts = serde_json::Map::new();
    for server in targets {
        let info: Value = sqlx::query_scalar("SELECT static_info FROM servers WHERE id=$1")
            .bind(server)
            .fetch_one(&mut **tx)
            .await?;
        let artifact = crate::agent::runtime_artifact(state, &info).await?;
        artifacts.insert(
            server.to_string(),
            serde_json::to_value(artifact).map_err(anyhow::Error::from)?,
        );
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO singbox_runtime_rollouts(id,administrator_id,runtime_version,targets,batch_size,created_at,updated_at,artifact_snapshot) VALUES($1,$2,$3,$4,$5,$6,$6,$7)").bind(id).bind(administrator).bind(version).bind(targets).bind(batch_size).bind(now_timestamp()).bind(json!(artifacts)).execute(&mut **tx).await?;
    Ok(id)
}

/// The rollout's targets, to check the administrator's server scope.
pub async fn targets_of(pool: &PgPool, id: Uuid) -> ApiResult<Vec<i64>> {
    sqlx::query_scalar("SELECT targets FROM singbox_runtime_rollouts WHERE id=$1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Locks the rollout for dispatching its next batch.
pub async fn lock_tx(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<Rollout> {
    let row=sqlx::query("SELECT targets,batch_size,dispatched_count,paused,artifact_snapshot FROM singbox_runtime_rollouts WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut **tx).await?.ok_or(ApiError::NotFound)?;
    Ok(Rollout {
        targets: row.get("targets"),
        batch_size: row.get("batch_size"),
        dispatched_count: row.get("dispatched_count"),
        paused: row.get("paused"),
        artifact_snapshot: row.get("artifact_snapshot"),
    })
}

/// Checks that a target may join its batch now and returns the digest of its signed
/// artifact: a confirmed deployment preflight, the runtime enabled, the device online, not
/// retiring and able to run runtime operations and checkpoints, and its platform's signed
/// artifact still the one the rollout chose.
pub async fn require_target_ready_tx(
    state: &AppState,
    headers: &HeaderMap,
    tx: &mut Transaction<'_, Postgres>,
    rollout: &Rollout,
    server: i64,
) -> ApiResult<String> {
    super::preflight::require_confirmed_tx(state, headers, server, tx).await?;
    crate::servers::lock(tx, server).await?;
    crate::settings::require_enabled(tx, server).await?;
    let status = sqlx::query(&format!(
        "SELECT static_info,capabilities,last_seen,{} AS retiring FROM servers WHERE id=$1",
        crate::retirement::retiring_sql("$1")
    ))
    .bind(server)
    .fetch_one(&mut **tx)
    .await?;
    let caps: Value = status.get("capabilities");
    if status.get::<bool, _>("retiring")
        || !status
            .get::<Option<i64>, _>("last_seen")
            .is_some_and(|at| now_timestamp().saturating_sub(at) <= 60)
        || !caps.as_array().is_some_and(|v| {
            [
                apsp_protocol::RUNTIME_OPERATIONS_CAPABILITY,
                apsp_protocol::RUNTIME_CHECKPOINT_CAPABILITY,
            ]
            .iter()
            .all(|required| v.iter().any(|capability| capability == *required))
        })
    {
        return Err(ApiError::Conflict(format!(
            "服务器 #{server} 离线、退役或缺少运行时运维/实际配置核对能力"
        )));
    }
    let artifact =
        crate::agent::runtime_artifact(state, &status.get::<Value, _>("static_info")).await?;
    let selected: apsp_protocol::Artifact =
        serde_json::from_value(rollout.artifact_snapshot[server.to_string()].clone())
            .map_err(anyhow::Error::from)?;
    if selected.sha256 != artifact.sha256 {
        return Err(ApiError::Conflict(format!(
            "服务器 #{server} 的运行时平台或已签名制品已变化，固定候选不会自动替换"
        )));
    }
    Ok(artifact.sha256)
}

/// Adds a dispatched target with its read-only inspection; the revision it targets now is
/// the baseline its next publication passes.
pub async fn add_member_tx(
    tx: &mut Transaction<'_, Postgres>,
    rollout: Uuid,
    server: i64,
    inspect_request_id: Uuid,
    dispatched_at: i64,
    artifact_sha256: &str,
) -> ApiResult<()> {
    let baseline:i64=sqlx::query_scalar("SELECT COALESCE(MAX(target_rev),0) FROM server_module_status WHERE server_id=$1 AND module='singbox'").bind(server).fetch_one(&mut **tx).await?;
    sqlx::query("INSERT INTO singbox_runtime_rollout_members(rollout_id,server_id,baseline_revision,inspect_request_id,dispatched_at,artifact_sha256) VALUES($1,$2,$3,$4,$5,$6)").bind(rollout).bind(server).bind(baseline).bind(inspect_request_id).bind(dispatched_at).bind(artifact_sha256).execute(&mut **tx).await?;
    Ok(())
}

/// Records a dispatched batch: its servers publish again, now with the rollout's runtime.
pub async fn batch_dispatched_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    servers: &[i64],
    dispatched_count: usize,
) -> ApiResult<()> {
    crate::business::mark_dirty(tx, servers).await?;
    sqlx::query(
        "UPDATE singbox_runtime_rollouts SET dispatched_count=$2,updated_at=$3 WHERE id=$1",
    )
    .bind(id)
    .bind(dispatched_count as i32)
    .bind(now_timestamp())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// The members whose inspection result has not reached the rollout: of one rollout, or of
/// every rollout.
pub async fn pending_inspections_tx(
    tx: &mut Transaction<'_, Postgres>,
    rollout: Option<Uuid>,
) -> ApiResult<Vec<PendingInspection>> {
    let rows: Vec<(Uuid, i64, Uuid)> = sqlx::query_as("SELECT rollout_id,server_id,inspect_request_id FROM singbox_runtime_rollout_members WHERE ($1::uuid IS NULL OR rollout_id=$1) AND inspection_result IS NULL")
        .bind(rollout).fetch_all(&mut **tx).await?;
    Ok(rows
        .into_iter()
        .map(|(rollout, server, request_id)| PendingInspection {
            rollout,
            server,
            request_id,
        })
        .collect())
}

/// Records the result of a member's inspection, unless one reached it already.
pub async fn record_inspection_tx(
    tx: &mut Transaction<'_, Postgres>,
    inspection: &PendingInspection,
    result: &Value,
) -> ApiResult<()> {
    sqlx::query("UPDATE singbox_runtime_rollout_members SET inspection_result=$4 WHERE rollout_id=$1 AND server_id=$2 AND inspect_request_id=$3 AND inspection_result IS NULL")
        .bind(inspection.rollout).bind(inspection.server).bind(inspection.request_id).bind(result).execute(&mut **tx).await?;
    Ok(())
}

/// The rollout's members with what each device confirmed, from the inspection results
/// recorded so far; the rollout completes once every target confirmed the version.
pub async fn members_tx(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<Vec<Value>> {
    let rows=sqlx::query("SELECT r.server_id,r.baseline_revision,r.artifact_sha256,r.dispatched_at,r.inspection_result AS result,m.target_rev,m.applied_rev,m.healthy,s.dirty_at,f.runtime_version,f.artifact_sha256 AS observed_artifact_sha256 FROM singbox_runtime_rollout_members r JOIN servers s ON s.id=r.server_id LEFT JOIN server_module_status m ON m.server_id=r.server_id AND m.module='singbox' LEFT JOIN singbox_runtime_manifest_facts f ON f.server_id=r.server_id AND f.module='singbox' AND f.revision=m.applied_rev WHERE r.rollout_id=$1 ORDER BY r.dispatched_at,r.server_id").bind(id).fetch_all(&mut **tx).await?;
    let mut result = Vec::new();
    for row in rows {
        let receipt: Option<RuntimeOperationResult> = row
            .get::<Option<Value>, _>("result")
            .map(serde_json::from_value)
            .transpose()
            .map_err(anyhow::Error::from)?;
        let checkpoint_confirmed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_module_checkpoints c JOIN runtime_control_receipts f ON f.request_id=c.checkpoint_request_id JOIN runtime_deployment_bindings b ON b.server_id=c.server_id AND b.module=c.module AND b.rev=$2 WHERE c.server_id=$1 AND c.module='singbox' AND c.verified_at>=$3 AND c.verified_at>$4-300 AND f.outcome='verified' AND c.checkpoint_json->'healthy'='true' AND c.checkpoint_json->'binding'->>'binding_digest'=b.binding_digest AND c.checkpoint_json->'binding'->>'bundle_sha256'=b.bundle_sha256 AND c.checkpoint_json->'binding'->>'deployment_id'=b.deployment_id::text)").bind(row.get::<i64,_>("server_id")).bind(row.get::<Option<i64>,_>("applied_rev")).bind(row.get::<i64,_>("dispatched_at")).bind(now_timestamp()).fetch_one(&mut **tx).await?;
        let confirmed = checkpoint_confirmed
            && receipt.as_ref().is_some_and(|r| {
                r.error.is_none()
                    && r.snapshot.as_ref().is_some_and(|s| {
                        s.healthy == Some(true)
                            && s.observed_at >= row.get::<i64, _>("dispatched_at")
                            && s.applied_revision.map(|v| v as i64)
                                == row.get::<Option<i64>, _>("target_rev")
                    })
            })
            && row.get::<Option<bool>, _>("healthy") == Some(true)
            && row.get::<Option<i64>, _>("target_rev") == row.get::<Option<i64>, _>("applied_rev")
            && row.get::<Option<i64>, _>("dirty_at").is_none()
            && row.get::<Option<String>, _>("runtime_version").as_deref()
                == Some(crate::RUNTIME_VERSION)
            && row
                .get::<Option<String>, _>("observed_artifact_sha256")
                .as_deref()
                == Some(row.get::<String, _>("artifact_sha256").as_str());
        result.push(json!({"server_id":row.get::<i64,_>("server_id"),"dispatched_at":row.get::<i64,_>("dispatched_at"),"confirmed":confirmed,"checkpoint_confirmed":checkpoint_confirmed,"candidate_artifact_sha256":row.get::<String,_>("artifact_sha256"),"state":if confirmed{"device_confirmed"}else if receipt.as_ref().is_some_and(|r|r.error.is_some()){"failed"}else{"awaiting_confirmation"},"target_revision":row.get::<Option<i64>,_>("target_rev"),"applied_revision":row.get::<Option<i64>,_>("applied_rev")}));
    }
    if !result.is_empty() && result.iter().all(|member| member["confirmed"] == true) {
        sqlx::query("UPDATE singbox_runtime_rollouts SET completed_at=$2,updated_at=$2 WHERE id=$1 AND completed_at IS NULL AND dispatched_count=cardinality(targets)").bind(id).bind(now_timestamp()).execute(&mut **tx).await?;
    }
    Ok(result)
}

/// The 50 latest rollouts as stored.
pub async fn recent_tx(tx: &mut Transaction<'_, Postgres>) -> ApiResult<Vec<Value>> {
    Ok(sqlx::query_scalar(
        "SELECT to_jsonb(r) FROM singbox_runtime_rollouts r ORDER BY created_at DESC LIMIT 50",
    )
    .fetch_all(&mut **tx)
    .await?)
}

/// Pauses or resumes a rollout; operations already dispatched keep running.
pub async fn set_paused_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    paused: bool,
) -> ApiResult<()> {
    let changed =
        sqlx::query("UPDATE singbox_runtime_rollouts SET paused=$2,updated_at=$3 WHERE id=$1")
            .bind(id)
            .bind(paused)
            .bind(now_timestamp())
            .execute(&mut **tx)
            .await?;
    if changed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

/// Locks a member of the rollout before its inspection is replaced.
pub async fn lock_member_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    server: i64,
) -> ApiResult<()> {
    let member:Option<i64>=sqlx::query_scalar("SELECT server_id FROM singbox_runtime_rollout_members WHERE rollout_id=$1 AND server_id=$2 FOR UPDATE").bind(id).bind(server).fetch_optional(&mut **tx).await?;
    member.ok_or(ApiError::NotFound)?;
    Ok(())
}

/// Replaces a member's inspection with a new one, whose result has yet to arrive.
pub async fn reinspect_member_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    server: i64,
    inspect_request_id: Uuid,
    dispatched_at: i64,
) -> ApiResult<()> {
    sqlx::query("UPDATE singbox_runtime_rollout_members SET inspect_request_id=$3,inspection_result=NULL,dispatched_at=$4 WHERE rollout_id=$1 AND server_id=$2").bind(id).bind(server).bind(inspect_request_id).bind(dispatched_at).execute(&mut **tx).await?;
    Ok(())
}

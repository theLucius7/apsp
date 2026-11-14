//! What a deployment-automation step may do with the proxy runtime: the step's requirement,
//! the fixed failed deployment it retries (the candidate), and whether a saved candidate, the
//! device's checkpoints and the read-only check of an unknown result still match the current
//! target. Operations runs the step and records it (its runtime operations and job history);
//! these functions answer whether a step is legal and what it targets.

use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use apsp_protocol::{
    Artifact, RuntimeBinding, RuntimeCheckpointRequest, RuntimeOperation, RuntimeOperationRequest,
    now_timestamp,
};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

const VERSION: &str = crate::RUNTIME_VERSION;
const MODULE: &str = "singbox";
/// What the Agent supports before it retries a fixed deployment and proves the result.
const RETRY_CAPABILITIES: [&str; 3] = [
    apsp_protocol::RUNTIME_OPERATIONS_CAPABILITY,
    apsp_protocol::RUNTIME_CHECKPOINT_CAPABILITY,
    apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY,
];

/// Runtime version and capability that an operations step on this runtime requires.
pub struct RuntimeStepRequirement {
    pub version: &'static str,
    pub capability: &'static str,
}

pub fn runtime_step_policy(kind: &str) -> Option<RuntimeStepRequirement> {
    (kind == "singbox_retry_deployment").then_some(RuntimeStepRequirement {
        version: VERSION,
        capability: "proxy:write",
    })
}

fn conflict(message: &str) -> ApiError {
    ApiError::Conflict(message.into())
}

fn retry_capable(capabilities: &Value) -> bool {
    capabilities.as_array().is_some_and(|values| {
        RETRY_CAPABILITIES
            .iter()
            .all(|required| values.iter().any(|value| value.as_str() == Some(*required)))
    })
}

async fn fixed_binding(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    revision: i64,
    hash: &str,
) -> ApiResult<RuntimeBinding> {
    let proposed = RuntimeBinding::new(Uuid::new_v4(), MODULE.into(), revision as u64, hash.into());
    if !proposed.valid() {
        return Err(conflict("已发布部署身份或摘要无效"));
    }
    sqlx::query("INSERT INTO runtime_deployment_bindings(deployment_id,server_id,module,rev,bundle_sha256,binding_digest) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(server_id,module,rev) DO NOTHING")
        .bind(proposed.deployment_id).bind(server).bind(MODULE).bind(revision).bind(hash).bind(&proposed.binding_digest).execute(&mut **tx).await?;
    let row = sqlx::query("SELECT deployment_id,bundle_sha256,binding_digest FROM runtime_deployment_bindings WHERE server_id=$1 AND module=$2 AND rev=$3")
        .bind(server).bind(MODULE).bind(revision).fetch_one(&mut **tx).await?;
    let binding = RuntimeBinding {
        deployment_id: row.get("deployment_id"),
        module: MODULE.into(),
        revision: revision as u64,
        bundle_sha256: row.get("bundle_sha256"),
        binding_digest: row.get("binding_digest"),
    };
    if !binding.valid() || binding.bundle_sha256 != hash {
        return Err(conflict("已发布部署的固定身份发生变化"));
    }
    Ok(binding)
}

fn artifact_identity(state: &AppState, artifact: &Artifact) -> ApiResult<Value> {
    let proof = artifact
        .proof
        .as_ref()
        .ok_or_else(|| conflict("运行时缺少独立签名证明"))?;
    let keys = state
        .release_keys
        .as_deref()
        .ok_or_else(|| conflict("面板缺少可信发布公钥"))?;
    let verified = apsp_protocol::release::verify_release(proof, keys)
        .map_err(|_| conflict("运行时签名证明校验失败"))?;
    let arch = artifact
        .url
        .rsplit('/')
        .next()
        .ok_or_else(|| conflict("签名制品平台无效"))?;
    let entry = verified
        .artifact("sing-box", VERSION, arch)
        .map_err(|_| conflict("签名运行时版本或平台不匹配"))?;
    if entry.sha256() != artifact.sha256 {
        return Err(conflict("运行时制品摘要与签名发布不匹配"));
    }
    Ok(
        json!({"sha256":artifact.sha256,"arch":arch,"bytes":entry.metadata().archive_size.to_string(),"version":VERSION,"signature_verified":true,"payload_verified":true,"source_repo":verified.metadata().source_repo,"release_tag":verified.metadata().tag}),
    )
}

pub async fn automation_candidate(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<Value> {
    crate::servers::lock(tx, server).await?;
    crate::settings::require_enabled(tx, server).await?;
    let row = sqlx::query(&format!("SELECT s.static_info,s.capabilities,s.last_seen,{} AS pending,m.target_rev,m.last_result_rev,m.last_error,m.healthy,m.updated_at,d.bundle,d.bundle_sha256,(SELECT MAX(rev) FROM deployments WHERE server_id=$1 AND module='singbox') AS latest_rev FROM servers s JOIN server_module_status m ON m.server_id=s.id AND m.module='singbox' JOIN deployments d ON d.server_id=s.id AND d.module=m.module AND d.rev=m.target_rev WHERE s.id=$1 FOR UPDATE OF m", crate::publications::pending_sql("s.id")))
        .bind(server).fetch_optional(&mut **tx).await?.ok_or_else(|| conflict("尚无已发布且失败的 sing-box 部署"))?;
    let now = now_timestamp();
    if !retry_capable(&row.get::<Value, _>("capabilities"))
        || !row
            .get::<Option<i64>, _>("last_seen")
            .is_some_and(|seen| (0..=60).contains(&now.saturating_sub(seen)))
    {
        return Err(conflict(
            "此步骤需要在线 Agent、运行时运维、实际配置核对及制品验签能力",
        ));
    }
    let revision: i64 = row.get("target_rev");
    if revision <= 0
        || row.get::<bool, _>("pending")
        || row.get::<Option<i64>, _>("latest_rev") != Some(revision)
        || row.get::<i64, _>("last_result_rev") != revision
        || row.get::<bool, _>("healthy")
        || row
            .get::<Option<String>, _>("last_error")
            .is_none_or(|error| error.is_empty())
    {
        return Err(conflict(
            "仅可重试稳定的、设备已明确回报失败的当前目标；新版本发布需使用代理业务发布流程",
        ));
    }
    let hash: String = row.get("bundle_sha256");
    if crate::auth::hash_token(&row.get::<String, _>("bundle")) != hash {
        return Err(conflict("已发布配置包完整性校验失败"));
    }
    let artifact =
        crate::agent::runtime_artifact(state, &row.get::<Value, _>("static_info")).await?;
    let identity = artifact_identity(state, &artifact)?;
    let pinned: Vec<Value> = sqlx::query_scalar("SELECT DISTINCT r.artifact FROM singbox_path_deployment_dependencies d JOIN singbox_chain_runtime_requirements r ON r.chain_id=d.chain_id AND r.generation=d.generation AND r.server_id=d.server_id WHERE d.server_id=$1 AND d.module='singbox' AND d.revision=$2")
        .bind(server).bind(revision).fetch_all(&mut **tx).await?;
    if pinned
        .iter()
        .any(|pin| pin["sha256"].as_str() != Some(artifact.sha256.as_str()))
    {
        return Err(conflict(
            "路径依赖与当前签名制品不一致，不能变更已有部署的运行时身份",
        ));
    }
    sqlx::query("INSERT INTO singbox_runtime_manifest_facts(server_id,module,revision,runtime_version,artifact_sha256,artifact) VALUES($1,'singbox',$2,$3,$4,$5) ON CONFLICT DO NOTHING")
        .bind(server).bind(revision).bind(VERSION).bind(&artifact.sha256).bind(serde_json::to_value(&artifact).map_err(anyhow::Error::from)?).execute(&mut **tx).await?;
    let same: bool = sqlx::query_scalar("SELECT runtime_version=$3 AND artifact_sha256=$4 FROM singbox_runtime_manifest_facts WHERE server_id=$1 AND module='singbox' AND revision=$2")
        .bind(server).bind(revision).bind(VERSION).bind(&artifact.sha256).fetch_one(&mut **tx).await?;
    if !same {
        return Err(conflict("此部署已固定另一运行时制品，不能覆盖原始事实"));
    }
    let binding = fixed_binding(tx, server, revision, &hash).await?;
    Ok(
        json!({"kind":"singbox_retry_deployment","module":MODULE,"runtime_version":VERSION,"binding":binding,"artifact":identity,"expected_failure_revision":revision,"target_status_updated_at":row.get::<i64, _>("updated_at"),"changes_business_configuration":false,"retry_unknown_result":false}),
    )
}

/// The runtime operation that retries the candidate's fixed deployment, valid until
/// `expires` and at most 600 seconds after `now`.
pub fn retry_request(
    candidate: &Value,
    now: i64,
    expires: i64,
) -> ApiResult<RuntimeOperationRequest> {
    let binding: RuntimeBinding =
        serde_json::from_value(candidate["binding"].clone()).map_err(anyhow::Error::from)?;
    let request = RuntimeOperationRequest {
        id: Uuid::new_v4(),
        module: MODULE.into(),
        operation: RuntimeOperation::RetryDeployment,
        expected_revision: Some(binding.revision),
        session_filter: None,
        requested_at: now,
        expires_at: expires.min(now + 600),
    };
    if !request.valid() {
        return Err(ApiError::BadRequest(
            "运行时重试时限无效，最长为 600 秒".into(),
        ));
    }
    Ok(request)
}

/// Whether the device answered a checkpoint of the runtime requested at or after `since`
/// as verified and healthy. An interrupted runtime operation stays unknown until such a
/// read; without `since` there is none.
pub async fn checkpoint_verified_since(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    since: Option<i64>,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_control_requests q JOIN runtime_control_receipts r ON r.request_id=q.request_id WHERE q.server_id=$1 AND q.module='singbox' AND q.kind='checkpoint' AND q.created_at>=$2 AND r.outcome='verified' AND r.result_json->'observed'->'healthy'='true'::jsonb)")
        .bind(server).bind(since).fetch_one(&mut **tx).await?)
}

/// Whether a queued retry may go to the server now: the runtime is enabled there and the
/// Agent supports runtime operations, checkpoints and signed artifacts.
pub async fn retry_target_ready(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<bool> {
    if !crate::settings::is_enabled(tx, server).await? {
        return Ok(false);
    }
    let caps: Value =
        sqlx::query_scalar("SELECT capabilities FROM servers WHERE id=$1 AND deleted_at IS NULL")
            .bind(server)
            .fetch_one(&mut **tx)
            .await?;
    Ok(retry_capable(&caps))
}

/// Whether a saved candidate still names the server's current target and its fixed signed
/// artifact.
pub async fn candidate_is_current(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    candidate: &Value,
) -> ApiResult<bool> {
    let binding: RuntimeBinding =
        serde_json::from_value(candidate["binding"].clone()).map_err(anyhow::Error::from)?;
    if !binding.valid()
        || binding.module != MODULE
        || !crate::runtime_control::target_is_current(tx, server, &binding).await?
    {
        return Ok(false);
    }
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_runtime_manifest_facts WHERE server_id=$1 AND module='singbox' AND revision=$2 AND runtime_version='1.14.4-apsp.1' AND artifact_sha256=$3)")
        .bind(server).bind(binding.revision as i64).bind(candidate["artifact"]["sha256"].as_str().unwrap_or("")).fetch_one(&mut **tx).await?)
}

/// What the device confirmed about a retry it reported finished without error.
pub enum RetryConfirmation {
    /// The saved candidate, the current target or its fixed artifact no longer match the
    /// request; the result stays uncertain.
    Mismatch,
    /// A fresh verified checkpoint shows the retried binding applied and healthy.
    Confirmed,
    /// Nothing confirms the retried binding yet; a checkpoint has to ask the device.
    Unconfirmed(RuntimeBinding),
}

/// Compares a retry the device finished without error with the current target and the
/// device's checkpoints since `dispatched_at`.
pub async fn retry_confirmation(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    candidate: &Value,
    request: &RuntimeOperationRequest,
    dispatched_at: i64,
    now: i64,
) -> ApiResult<RetryConfirmation> {
    let binding: RuntimeBinding =
        serde_json::from_value(candidate["binding"].clone()).map_err(anyhow::Error::from)?;
    let current = crate::runtime_control::target_is_current(tx, server, &binding).await?;
    let facts: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_runtime_manifest_facts WHERE server_id=$1 AND module='singbox' AND revision=$2 AND runtime_version='1.14.4-apsp.1' AND artifact_sha256=$3)")
        .bind(server).bind(binding.revision as i64).bind(candidate["artifact"]["sha256"].as_str().unwrap_or("")).fetch_one(&mut **tx).await?;
    let matching_request = binding.valid()
        && binding.module == request.module
        && Some(binding.revision) == request.expected_revision;
    if !(current && facts && matching_request) {
        return Ok(RetryConfirmation::Mismatch);
    }
    let confirmed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_module_checkpoints c JOIN runtime_control_receipts r ON r.request_id=c.checkpoint_request_id JOIN server_module_status m ON m.server_id=c.server_id AND m.module=c.module WHERE c.server_id=$1 AND c.module='singbox' AND c.verified_at>=$2 AND c.verified_at>$3-300 AND r.outcome='verified' AND c.checkpoint_json->'healthy'='true'::jsonb AND c.checkpoint_json->'binding'=$4 AND m.target_rev=$5 AND m.applied_rev=$5 AND m.healthy)")
        .bind(server).bind(dispatched_at).bind(now).bind(serde_json::to_value(&binding).map_err(anyhow::Error::from)?).bind(binding.revision as i64).fetch_one(&mut **tx).await?;
    Ok(if confirmed {
        RetryConfirmation::Confirmed
    } else {
        RetryConfirmation::Unconfirmed(binding)
    })
}

/// The read-only checkpoint that confirms a retried binding: a pending checkpoint sent for
/// it since `dispatched_at` is reused, an expired one is replaced. `None` while another
/// pending checkpoint, older or for another binding, is still out.
pub async fn retry_checkpoint(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    binding: &RuntimeBinding,
    now: i64,
    dispatched_at: i64,
) -> ApiResult<Option<Uuid>> {
    sqlx::query("UPDATE runtime_control_requests SET state='expired' WHERE server_id=$1 AND module='singbox' AND kind='checkpoint' AND state='pending' AND expires_at<=$2")
        .bind(server).bind(now).execute(&mut **tx).await?;
    let pending = sqlx::query("SELECT request_id,request_json,created_at FROM runtime_control_requests WHERE server_id=$1 AND module='singbox' AND kind='checkpoint' AND state='pending'").bind(server).fetch_optional(&mut **tx).await?;
    let checkpoint = if let Some(pending) = pending {
        let saved: RuntimeCheckpointRequest =
            serde_json::from_value(pending.get("request_json")).map_err(anyhow::Error::from)?;
        if saved.expected != *binding || pending.get::<i64, _>("created_at") < dispatched_at {
            return Ok(None);
        }
        saved
    } else {
        let checkpoint = RuntimeCheckpointRequest {
            request_id: Uuid::new_v4(),
            expected: binding.clone(),
            expires_at: now + 120,
        };
        sqlx::query("INSERT INTO runtime_control_requests(request_id,server_id,module,kind,request_digest,request_json,created_at,expires_at) VALUES($1,$2,'singbox','checkpoint',$3,$4,$5,$6)")
            .bind(checkpoint.request_id).bind(server).bind(checkpoint.digest().map_err(anyhow::Error::from)?).bind(serde_json::to_value(&checkpoint).map_err(anyhow::Error::from)?).bind(now).bind(checkpoint.expires_at).execute(&mut **tx).await?;
        checkpoint
    };
    Ok(Some(checkpoint.request_id))
}

/// The fixed deployment of a dispatched retry whose result is unknown.
pub struct FixedDeployment {
    pub binding: RuntimeBinding,
    pub artifact_sha256: String,
}

/// The fixed deployment a saved candidate names for a retry whose result is unknown, while
/// it is still the server's current target with its fixed signed artifact; only then can a
/// read-only check reconcile the result.
pub async fn unknown_retry_target(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    candidate: &Value,
    request: &RuntimeOperationRequest,
) -> ApiResult<FixedDeployment> {
    let binding: RuntimeBinding =
        serde_json::from_value(candidate["binding"].clone()).map_err(anyhow::Error::from)?;
    let sha = candidate["artifact"]["sha256"]
        .as_str()
        .ok_or_else(|| conflict("原签名制品摘要缺失"))?
        .to_owned();
    if !binding.valid()
        || binding.module != MODULE
        || Some(binding.revision) != request.expected_revision
        || !crate::runtime_control::target_is_current(tx, server, &binding).await?
    {
        return Err(conflict(
            "当前目标已不匹配原固定部署，需要受控恢复，不能用其他版本证明原请求已恢复",
        ));
    }
    let same: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM singbox_runtime_manifest_facts WHERE server_id=$1 AND module='singbox' AND revision=$2 AND runtime_version='1.14.4-apsp.1' AND artifact_sha256=$3)").bind(server).bind(binding.revision as i64).bind(&sha).fetch_one(&mut **tx).await?;
    if !same {
        return Err(conflict("原固定签名制品身份已变化，不能核对解锁"));
    }
    Ok(FixedDeployment {
        binding,
        artifact_sha256: sha,
    })
}

/// Requests a read-only checkpoint of a fixed deployment whose retry result is unknown, and
/// marks the runtime unhealthy until the device answers.
pub async fn request_unknown_checkpoint(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    binding: RuntimeBinding,
    now: i64,
) -> ApiResult<RuntimeCheckpointRequest> {
    // A new read supersedes older reads, never the original deployment mutation.
    sqlx::query("UPDATE runtime_control_requests SET state='expired' WHERE server_id=$1 AND module='singbox' AND kind='checkpoint' AND state='pending'").bind(server).execute(&mut **tx).await?;
    let checkpoint = RuntimeCheckpointRequest {
        request_id: Uuid::new_v4(),
        expected: binding,
        expires_at: now + 120,
    };
    sqlx::query("INSERT INTO runtime_control_requests(request_id,server_id,module,kind,request_digest,request_json,created_at,expires_at) VALUES($1,$2,'singbox','checkpoint',$3,$4,$5,$6)")
        .bind(checkpoint.request_id).bind(server).bind(checkpoint.digest().map_err(anyhow::Error::from)?).bind(serde_json::to_value(&checkpoint).map_err(anyhow::Error::from)?).bind(now).bind(checkpoint.expires_at).execute(&mut **tx).await?;
    sqlx::query("UPDATE server_module_status SET healthy=false,updated_at=$2 WHERE server_id=$1 AND module='singbox'").bind(server).bind(now).execute(&mut **tx).await?;
    Ok(checkpoint)
}

/// Whether the device answered that read-only checkpoint, requested at or after `cutoff`,
/// within the last five minutes as verified and healthy, with the fixed deployment applied
/// as the current target.
pub async fn unknown_checkpoint_verified(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    checkpoint: Uuid,
    cutoff: i64,
    binding: &RuntimeBinding,
    now: i64,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_control_requests q JOIN runtime_control_receipts r ON r.request_id=q.request_id JOIN runtime_module_checkpoints c ON c.checkpoint_request_id=r.request_id JOIN server_module_status m ON m.server_id=c.server_id AND m.module=c.module WHERE q.request_id=$1 AND q.server_id=$2 AND q.module='singbox' AND q.kind='checkpoint' AND q.created_at>=$3 AND q.request_json->'expected'=$4 AND r.outcome='verified' AND r.received_at>$5-300 AND c.verified_at>$5-300 AND c.checkpoint_json->'binding'=$4 AND c.checkpoint_json->'healthy'='true'::jsonb AND m.healthy AND m.applied_rev=$6 AND m.target_rev=$6)")
        .bind(checkpoint).bind(server).bind(cutoff).bind(serde_json::to_value(binding).map_err(anyhow::Error::from)?).bind(now).bind(binding.revision as i64).fetch_one(&mut **tx).await?)
}

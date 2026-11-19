//! What a deployment-automation step may do with the proxy runtime: the step's requirement,
//! the fixed failed deployment it retries (the candidate), and whether a saved candidate, the
//! device's checkpoints and the read-only check of an unknown result still match the current
//! target and the running self-contained Agent bundle. Operations runs the step and records it
//! (its runtime operations and job history); these functions answer whether a step is legal and
//! what it targets.

use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use apsp_protocol::{
    RuntimeBinding, RuntimeCheckpointRequest, RuntimeOperation, RuntimeOperationRequest,
    now_timestamp,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

const VERSION: &str = crate::RUNTIME_VERSION;
const MODULE: &str = "singbox";
const COMPONENT: &str = "sing-box";
const STATIC_INFO_MAX_AGE_SECS: i64 = 600;

/// Capabilities required beyond self-contained sing-box support before retrying a fixed
/// failed deployment and proving the result.
const RETRY_CAPABILITIES: [&str; 2] = [
    apsp_protocol::RUNTIME_OPERATIONS_CAPABILITY,
    apsp_protocol::RUNTIME_CHECKPOINT_CAPABILITY,
];

/// Capability that an operations step on this runtime requires.
pub struct RuntimeStepRequirement {
    pub capability: &'static str,
}

pub fn runtime_step_policy(kind: &str) -> Option<RuntimeStepRequirement> {
    (kind == "singbox_retry_deployment").then_some(RuntimeStepRequirement {
        capability: "proxy:write",
    })
}

/// Verified running Agent package and embedded sing-box identity frozen into a retry candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenAgentBundle {
    pub agent_version: String,
    pub release_tag: String,
    pub target: String,
    pub source_repo: String,
    pub agent_sha256: String,
    pub agent_bytes: u64,
    pub component: String,
    pub component_version: String,
    pub component_sha256: String,
}

impl From<crate::releases::VerifiedRunningComponent> for FrozenAgentBundle {
    fn from(verified: crate::releases::VerifiedRunningComponent) -> Self {
        Self {
            agent_version: verified.agent_version,
            release_tag: verified.release_tag,
            target: verified.target,
            source_repo: verified.source_repo,
            agent_sha256: verified.agent_sha256,
            agent_bytes: verified.agent_bytes,
            component: verified.component,
            component_version: verified.component_version,
            component_sha256: verified.component_sha256,
        }
    }
}

impl FrozenAgentBundle {
    fn valid(&self) -> bool {
        !self.agent_version.is_empty()
            && self.release_tag == format!("agent-v{}", self.agent_version)
            && apsp_protocol::platform::ARTIFACT_TARGETS.contains(&self.target.as_str())
            && self.source_repo == apsp_protocol::release::RELEASE_SOURCE_REPO
            && apsp_protocol::bundled::valid_sha256(&self.agent_sha256)
            && self.agent_bytes > 0
            && self.component == COMPONENT
            && self.component_version == VERSION
            && apsp_protocol::bundled::valid_sha256(&self.component_sha256)
    }
}

/// Whether `candidate` uses the retired pre-consolidation standalone artifact format.
pub fn is_legacy_candidate(candidate: &Value) -> bool {
    candidate.get("artifact").is_some() || candidate.get("runtime_version").is_some()
}

fn conflict(message: &str) -> ApiError {
    ApiError::Conflict(message.into())
}

fn retry_capable(capabilities: &Value) -> bool {
    crate::agent::supports_self_contained_singbox(capabilities)
        && capabilities.as_array().is_some_and(|values| {
            RETRY_CAPABILITIES
                .iter()
                .all(|required| values.iter().any(|value| value.as_str() == Some(*required)))
        })
}

fn non_empty_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value[key].as_str().filter(|entry| !entry.is_empty())
}

fn frozen_bundle(candidate: &Value) -> Option<FrozenAgentBundle> {
    if is_legacy_candidate(candidate) {
        return None;
    }
    let bundle: FrozenAgentBundle =
        serde_json::from_value(candidate.get("agent_bundle")?.clone()).ok()?;
    bundle.valid().then_some(bundle)
}

async fn device_matches_frozen_bundle(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    expected: &FrozenAgentBundle,
    now: i64,
) -> ApiResult<bool> {
    if !expected.valid() {
        return Ok(false);
    }
    let Some(row) = sqlx::query(
        "SELECT static_info,static_info_received_at,capabilities,last_seen FROM servers WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(server)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(false);
    };
    let capabilities: Value = row.get("capabilities");
    let last_seen: Option<i64> = row.get("last_seen");
    let received_at: Option<i64> = row.get("static_info_received_at");
    if !retry_capable(&capabilities)
        || !last_seen.is_some_and(|seen| seen <= now + 60 && now.saturating_sub(seen) <= 60)
        || !received_at.is_some_and(|at| {
            at > 0 && at <= now + 60 && now.saturating_sub(at) <= STATIC_INFO_MAX_AGE_SECS
        })
    {
        return Ok(false);
    }
    let info: Value = row.get("static_info");
    let matches = info["agent_version"].as_str() == Some(expected.agent_version.as_str())
        && crate::releases::reported_agent_sha256(&info) == Some(expected.agent_sha256.as_str())
        && info["components"][COMPONENT].as_str() == Some(expected.component_version.as_str())
        && info["component_digests"][COMPONENT].as_str()
            == Some(expected.component_sha256.as_str())
        && crate::releases::agent_targets(&info).contains(&expected.target);
    Ok(matches)
}

async fn verified_current_bundle(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    now: i64,
) -> ApiResult<FrozenAgentBundle> {
    if !crate::settings::is_configured(tx, server).await? {
        return Err(conflict("当前服务器未配置受管流量服务，不能核对解锁原部署"));
    }
    let row = sqlx::query(
        "SELECT static_info,static_info_received_at,capabilities,last_seen FROM servers WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(server)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ApiError::NotFound)?;
    let capabilities: Value = row.get("capabilities");
    if !retry_capable(&capabilities) {
        return Err(conflict(
            "当前设备缺少一体化 Agent、内置 sing-box、运行时运维或实际配置核对能力",
        ));
    }
    let static_info: Value = row.get("static_info");
    let static_info_received_at: Option<i64> = row.get("static_info_received_at");
    let last_seen: Option<i64> = row.get("last_seen");
    let verified = crate::releases::verify_running_component(
        state,
        &static_info,
        static_info_received_at,
        last_seen,
        now,
        COMPONENT,
        VERSION,
    )
    .await?;
    Ok(FrozenAgentBundle::from(verified))
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

pub async fn automation_candidate(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<Value> {
    crate::servers::lock(tx, server).await?;
    crate::settings::require_configured(tx, server).await?;
    let row = sqlx::query(&format!("SELECT s.static_info,s.static_info_received_at,s.capabilities,s.last_seen,{} AS pending,m.target_rev,m.last_result_rev,m.last_error,m.healthy,m.updated_at,d.bundle,d.bundle_sha256,(SELECT MAX(rev) FROM deployments WHERE server_id=$1 AND module='singbox') AS latest_rev FROM servers s JOIN server_module_status m ON m.server_id=s.id AND m.module='singbox' JOIN deployments d ON d.server_id=s.id AND d.module=m.module AND d.rev=m.target_rev WHERE s.id=$1 AND s.deleted_at IS NULL FOR UPDATE OF m", crate::publications::pending_sql("s.id")))
        .bind(server).fetch_optional(&mut **tx).await?.ok_or_else(|| conflict("尚无已发布且失败的 sing-box 部署"))?;
    let now = now_timestamp();
    let last_seen: Option<i64> = row.get("last_seen");
    if !retry_capable(&row.get::<Value, _>("capabilities"))
        || !last_seen.is_some_and(|seen| seen <= now + 60 && now.saturating_sub(seen) <= 60)
    {
        return Err(conflict(
            "此步骤需要在线的一体化 Agent、内置 sing-box、运行时运维及实际配置核对能力",
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
    let static_info: Value = row.get("static_info");
    let static_info_received_at: Option<i64> = row.get("static_info_received_at");
    let verified = crate::releases::verify_running_component(
        state,
        &static_info,
        static_info_received_at,
        last_seen,
        now,
        COMPONENT,
        VERSION,
    )
    .await?;
    let agent_bundle = FrozenAgentBundle::from(verified);
    let observed_agent_version = non_empty_str(&static_info, "agent_version");
    let observed_runtime_version = crate::agent::reported_runtime_version(&static_info);
    let binding = fixed_binding(tx, server, revision, &hash).await?;
    Ok(json!({
        "kind": "singbox_retry_deployment",
        "module": MODULE,
        "binding": binding,
        "expected_failure_revision": revision,
        "target_status_updated_at": row.get::<i64, _>("updated_at"),
        "observed_agent_version": observed_agent_version,
        "observed_runtime_version": observed_runtime_version,
        "changes_business_configuration": false,
        "retry_unknown_result": false,
        "agent_bundle": agent_bundle,
    }))
}

/// The runtime operation that retries the candidate's fixed deployment, valid until
/// `expires` and at most 600 seconds after `now`.
pub fn retry_request(
    candidate: &Value,
    now: i64,
    expires: i64,
) -> ApiResult<RuntimeOperationRequest> {
    if frozen_bundle(candidate).is_none() {
        return Err(conflict("固定失败部署缺少有效的一体化 Agent 整包身份"));
    }
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

/// Whether a queued retry may go to the server now: the runtime is configured there and the
/// Agent supports self-contained sing-box, runtime operations, checkpoints and release proofs.
pub async fn retry_target_ready(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<bool> {
    if !crate::settings::is_configured(tx, server).await? {
        return Ok(false);
    }
    let Some(caps): Option<Value> =
        sqlx::query_scalar("SELECT capabilities FROM servers WHERE id=$1 AND deleted_at IS NULL")
            .bind(server)
            .fetch_optional(&mut **tx)
            .await?
    else {
        return Ok(false);
    };
    Ok(retry_capable(&caps))
}

/// Whether a saved candidate still names the server's current target and its frozen
/// verified Agent bundle and embedded sing-box component identity.
pub async fn candidate_is_current(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    candidate: &Value,
) -> ApiResult<bool> {
    let Some(bundle) = frozen_bundle(candidate) else {
        return Ok(false);
    };
    let Ok(binding) = serde_json::from_value::<RuntimeBinding>(candidate["binding"].clone()) else {
        return Ok(false);
    };
    if !binding.valid()
        || binding.module != MODULE
        || !crate::runtime_control::target_is_current(tx, server, &binding).await?
    {
        return Ok(false);
    }
    device_matches_frozen_bundle(tx, server, &bundle, now_timestamp()).await
}

/// What the device confirmed about a retry it reported finished without error.
pub enum RetryConfirmation {
    /// The saved candidate, the current target or its frozen Agent bundle no longer match the
    /// request; the result stays uncertain.
    Mismatch,
    /// A fresh verified checkpoint shows the retried binding applied and healthy.
    Confirmed,
    /// Nothing confirms the retried binding yet; a checkpoint has to ask the device.
    Unconfirmed(RuntimeBinding),
}

/// Compares a retry the device finished without error with the current target, the frozen
/// Agent bundle and the device's checkpoints since `dispatched_at`.
pub async fn retry_confirmation(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    candidate: &Value,
    request: &RuntimeOperationRequest,
    dispatched_at: i64,
    now: i64,
) -> ApiResult<RetryConfirmation> {
    let Some(bundle) = frozen_bundle(candidate) else {
        return Ok(RetryConfirmation::Mismatch);
    };
    let Ok(binding) = serde_json::from_value::<RuntimeBinding>(candidate["binding"].clone()) else {
        return Ok(RetryConfirmation::Mismatch);
    };
    let current = crate::runtime_control::target_is_current(tx, server, &binding).await?;
    let bundle_matches = crate::settings::is_configured(tx, server).await?
        && device_matches_frozen_bundle(tx, server, &bundle, now).await?;
    let matching_request = binding.valid()
        && binding.module == request.module
        && Some(binding.revision) == request.expected_revision;
    if !(current && bundle_matches && matching_request) {
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

/// The fixed deployment and verified running Agent bundle inspected for a dispatched retry
/// whose result is unknown.
pub struct FixedDeployment {
    pub binding: RuntimeBinding,
    pub inspected_agent_bundle: FrozenAgentBundle,
    pub legacy_candidate: bool,
}

/// The fixed deployment a saved candidate names for an already-dispatched retry whose result
/// is unknown, while it is still the server's current target and the server is running a
/// verified signed self-contained Agent bundle. New candidates must still match their frozen
/// Agent bundle; already-dispatched pre-consolidation candidates retain read-only checkpoint
/// reconciliation without converting or replaying the original mutation.
pub async fn unknown_retry_target(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    candidate: &Value,
    request: &RuntimeOperationRequest,
    now: i64,
) -> ApiResult<FixedDeployment> {
    if candidate["kind"].as_str() != Some("singbox_retry_deployment")
        || candidate["module"].as_str() != Some(MODULE)
    {
        return Err(conflict("原部署候选类型或模块无效，不能核对解锁"));
    }
    let binding: RuntimeBinding = serde_json::from_value(candidate["binding"].clone())
        .map_err(|_| conflict("原部署的固定配置身份缺失或无效，不能核对解锁"))?;
    if !binding.valid()
        || binding.module != MODULE
        || Some(binding.revision) != request.expected_revision
        || !crate::runtime_control::target_is_current(tx, server, &binding).await?
    {
        return Err(conflict(
            "当前目标已不匹配原固定部署，需要受控恢复，不能用其他版本证明原请求已恢复",
        ));
    }
    if is_legacy_candidate(candidate) {
        if !candidate["artifact"]["sha256"]
            .as_str()
            .is_some_and(apsp_protocol::bundled::valid_sha256)
        {
            return Err(conflict("原旧版部署候选缺少有效制品摘要，不能核对解锁"));
        }
        let inspected_agent_bundle = verified_current_bundle(state, tx, server, now).await?;
        return Ok(FixedDeployment {
            binding,
            inspected_agent_bundle,
            legacy_candidate: true,
        });
    }
    let original = frozen_bundle(candidate)
        .ok_or_else(|| conflict("原固定可信 Agent 整包身份缺失或无效，不能核对解锁"))?;
    let inspected_agent_bundle = verified_current_bundle(state, tx, server, now).await?;
    if inspected_agent_bundle != original {
        return Err(conflict(
            "原固定可信 Agent 整包或内置运行时身份已变化，不能核对解锁",
        ));
    }
    Ok(FixedDeployment {
        binding,
        inspected_agent_bundle,
        legacy_candidate: false,
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

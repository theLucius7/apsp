use super::{Envelope, invalid, receipts::canonical};
use crate::{
    diagnostic_tools::ipquality::{SOURCE_COMMIT, SOURCE_SHA256, TOOL_VERSION},
    diagnostics::service::{FrozenComponentIdentity, SectionContext},
    error::ApiResult,
};
use apsp_protocol::{DiagnosticSectionUpdate, bundled::valid_sha256};

pub(super) fn validate(
    context: &SectionContext<'_>,
    update: &DiagnosticSectionUpdate,
    envelope: &Envelope,
) -> ApiResult<Option<String>> {
    let frozen = FrozenComponentIdentity::from_job(context.job)
        .map_err(|_| invalid("派单冻结的签名组件身份格式无效"))?;
    let expected_sha256 = if let Some(frozen) = &frozen {
        if !context.live_verified
            || frozen.component != "ipquality"
            || frozen.component_version != TOOL_VERSION
            || context.job["options"]["expected_component_sha256"].as_str()
                != Some(frozen.component_sha256.as_str())
        {
            return Err(invalid("任务身份、固定来源、冻结签名组件或执行窗口不一致"));
        }
        frozen.component_sha256.as_str()
    } else {
        let Some(legacy_sha256) = context.job["artifact"]["sha256"]
            .as_str()
            .filter(|sha| valid_sha256(sha))
        else {
            return Err(invalid("任务缺少派单冻结的签名组件或历史证明"));
        };
        if context.live_verified
            || !context.job["artifact"]["proof"].is_object()
            || context.job["options"]
                .get("expected_component_sha256")
                .is_some()
        {
            return Err(invalid("历史证明收据格式无效或与当前签名组件契约混淆"));
        }
        legacy_sha256
    };
    if envelope.schema != 1
        || envelope.tool != "ipquality"
        || envelope.version != TOOL_VERSION
        || envelope.job_id != update.id
        || context.job["id"].as_str() != Some(update.id.to_string().as_str())
        || context.job["plugin"].as_str() != Some("ipquality")
        || context.job["version"].as_str() != Some(TOOL_VERSION)
        || !matches!(envelope.ip_version.as_str(), "4" | "6")
        || context.job["options"]["ip_version"].as_str() != Some(envelope.ip_version.as_str())
        || context.job["timeout_secs"].as_u64() != Some(300)
        || envelope.source_commit != SOURCE_COMMIT
        || envelope.source_sha256 != SOURCE_SHA256
        || !valid_sha256(&envelope.artifact_sha256)
        || envelope.artifact_sha256 != expected_sha256
        || envelope.started_at < context.created_at.saturating_sub(300)
        || envelope.started_at > context.expires_at
        || update.collected_at < envelope.started_at
        || update.collected_at > context.expires_at.saturating_add(300)
        || envelope.finished_at.is_some_and(|finished| {
            finished < envelope.started_at
                || finished > context.expires_at
                || finished > update.collected_at
        })
        || envelope
            .finished_at
            .is_some_and(|finished| finished.saturating_sub(envelope.started_at) > 360)
        || (envelope.finished_at.is_none()
            && update.collected_at.saturating_sub(envelope.started_at) > 360)
        || (update.complete && envelope.finished_at.is_none())
        || envelope.attempts.len() > 64
    {
        return Err(invalid("任务身份、固定来源、冻结签名组件或执行窗口不一致"));
    }
    let canonical_ip = envelope
        .egress_ip
        .as_ref()
        .and_then(|ip| canonical(ip, &envelope.ip_version));
    if envelope.egress_ip.is_some() && canonical_ip != envelope.egress_ip {
        return Err(invalid("出口 IP 不是已规范化的本次公网地址"));
    }
    if let Some(upstream) = &envelope.upstream
        && (!upstream.is_object()
            || upstream["Head"]["IP"]
                .as_str()
                .and_then(|ip| canonical(ip, &envelope.ip_version))
                != canonical_ip
            || canonical_ip.is_none()
            || upstream["Head"]["Version"].as_str() != Some("v2026-09-16"))
    {
        return Err(invalid("原工具 JSON 没有确认真实出口和固定版本"));
    }
    Ok(canonical_ip)
}

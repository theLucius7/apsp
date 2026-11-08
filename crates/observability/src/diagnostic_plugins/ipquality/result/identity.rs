use super::{Envelope, invalid, receipts::canonical};
use crate::{
    diagnostic_plugins::ipquality::{PLUGIN_VERSION, SOURCE_COMMIT, SOURCE_SHA256},
    diagnostics::service::SectionContext,
    error::ApiResult,
};
use apsp_protocol::DiagnosticSectionUpdate;

pub(super) fn validate(
    context: &SectionContext<'_>,
    update: &DiagnosticSectionUpdate,
    envelope: &Envelope,
) -> ApiResult<Option<String>> {
    if envelope.schema != 1
        || envelope.plugin != "ipquality"
        || envelope.version != PLUGIN_VERSION
        || envelope.job_id != update.id
        || context.job["id"].as_str() != Some(update.id.to_string().as_str())
        || context.job["plugin"].as_str() != Some("ipquality")
        || context.job["version"].as_str() != Some(PLUGIN_VERSION)
        || !matches!(envelope.ip_version.as_str(), "4" | "6")
        || context.job["options"]["ip_version"].as_str() != Some(envelope.ip_version.as_str())
        || context.job["timeout_secs"].as_u64() != Some(300)
        || envelope.source_commit != SOURCE_COMMIT
        || envelope.source_sha256 != SOURCE_SHA256
        || envelope.artifact_sha256.len() != 64
        || !envelope
            .artifact_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || context.job["artifact"]["sha256"].as_str() != Some(envelope.artifact_sha256.as_str())
        || context.job["artifact"]["proof"].is_null()
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
        return Err(invalid("任务身份、固定来源、签名制品或执行窗口不一致"));
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

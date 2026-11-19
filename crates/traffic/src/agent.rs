use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use apsp_protocol::{
    ModuleManifest, SELF_CONTAINED_AGENT_CAPABILITY, bundled::valid_version_segment,
};
use serde_json::Value;
use sqlx::Row;

/// Whether `capabilities` declares both the self-contained Agent contract and built-in sing-box.
pub fn supports_self_contained_singbox(capabilities: &Value) -> bool {
    capabilities.as_array().is_some_and(|values| {
        let has = |expected: &str| values.iter().any(|item| item.as_str() == Some(expected));
        has(SELF_CONTAINED_AGENT_CAPABILITY) && has("singbox")
    })
}

/// Extracts the currently applied/running `sing-box` version reported by the Agent in `static_info`.
/// Never falls back to `static_info.components["sing-box"]`, which remains unapplied before first start.
pub fn reported_runtime_version(static_info: &Value) -> Option<String> {
    static_info
        .get("runtime_version")
        .and_then(Value::as_str)
        .filter(|value| valid_version_segment(value))
        .map(str::to_owned)
}

/// Extracts the embedded `sing-box` payload version reported in `static_info.components["sing-box"]`.
pub fn embedded_runtime_version(static_info: &Value) -> Option<String> {
    static_info
        .get("components")
        .and_then(|components| components.get("sing-box"))
        .and_then(Value::as_str)
        .filter(|value| valid_version_segment(value))
        .map(str::to_owned)
}

pub async fn manifest_module(
    state: &AppState,
    server_id: i64,
    info: &Value,
) -> ApiResult<Option<ModuleManifest>> {
    let mut connection = state.pool.acquire().await?;
    if !crate::settings::is_configured_on(&mut connection, server_id).await? {
        return Ok(None);
    }
    let deployment = sqlx::query(
        "SELECT d.rev,d.bundle_sha256,s.capabilities \
         FROM deployments d \
         JOIN servers s ON s.id=d.server_id \
         LEFT JOIN server_module_status m ON m.server_id=d.server_id AND m.module=d.module \
         WHERE d.server_id=$1 \
           AND d.module='singbox' \
           AND d.rev>0 \
           AND COALESCE(m.target_rev,d.rev)=d.rev \
         ORDER BY d.rev DESC \
         LIMIT 1",
    )
    .bind(server_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(deployment) = deployment else {
        return Ok(None);
    };
    let revision: i64 = deployment.get("rev");
    let bundle_sha256: String = deployment.get("bundle_sha256");
    let capabilities: Value = deployment.get("capabilities");
    let below_floor: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_path_deployment_dependencies d JOIN singbox_chains c ON c.id=d.chain_id WHERE d.server_id=$1 AND d.module='singbox' AND d.revision=$2 AND d.route_active AND d.generation<c.minimum_generation)",
    )
    .bind(server_id)
    .bind(revision)
    .fetch_one(&mut *connection)
    .await?;
    if below_floor {
        return Err(ApiError::BadRequest(
            "此配置含低于设备已确认恢复边界的路径代数，等待发布安全的较高代数".into(),
        ));
    }
    if !supports_self_contained_singbox(&capabilities) {
        return Err(ApiError::Conflict(
            "当前 Agent 尚未声明内置 sing-box 一体化能力，请先升级 Agent；已有运行配置保持不变"
                .into(),
        ));
    }
    if let Some(component_version) = info
        .get("components")
        .and_then(|components| components.get("sing-box"))
        .and_then(Value::as_str)
        .filter(|version| !version.is_empty())
        && component_version != crate::RUNTIME_VERSION
    {
        return Err(ApiError::Conflict(format!(
            "当前 Agent 内置 sing-box 版本（{component_version}）与面板要求版本（{}）不一致，请升级 Agent",
            crate::RUNTIME_VERSION
        )));
    }
    Ok(Some(ModuleManifest {
        kernel_version: crate::RUNTIME_VERSION.into(),
        artifact: None,
        config_rev: revision as u64,
        bundle_url: format!(
            "{}/api/agent/v1/bundles/{revision}",
            state.config.public_url
        ),
        bundle_sha256,
        stats_listen: "127.0.0.1:18085".into(),
    }))
}

pub async fn bundle(state: &AppState, server_id: i64, rev: i64) -> ApiResult<String> {
    let mut connection = state.pool.acquire().await?;
    if !crate::settings::is_configured_on(&mut connection, server_id).await? {
        return Err(ApiError::NotFound);
    }
    let below_floor: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_path_deployment_dependencies d JOIN singbox_chains c ON c.id=d.chain_id WHERE d.server_id=$1 AND d.module='singbox' AND d.revision=$2 AND d.route_active AND d.generation<c.minimum_generation)",
    )
    .bind(server_id)
    .bind(rev)
    .fetch_one(&mut *connection)
    .await?;
    if below_floor {
        return Err(ApiError::BadRequest(
            "此配置含低于设备已确认恢复边界的路径代数，等待发布安全的较高代数".into(),
        ));
    }
    sqlx::query_scalar(
        "SELECT d.bundle FROM deployments d LEFT JOIN server_module_status m ON m.server_id=d.server_id AND m.module=d.module WHERE d.server_id=$1 AND d.module='singbox' AND d.rev=$2 AND COALESCE(m.target_rev,d.rev)>0",
    )
    .bind(server_id)
    .bind(rev)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(ApiError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn self_contained_singbox_requires_both_capabilities() {
        assert!(!supports_self_contained_singbox(&json!(["singbox"])));
        assert!(!supports_self_contained_singbox(&json!([
            SELF_CONTAINED_AGENT_CAPABILITY
        ])));
        assert!(supports_self_contained_singbox(&json!([
            SELF_CONTAINED_AGENT_CAPABILITY,
            "singbox"
        ])));
    }

    #[test]
    fn reported_and_embedded_runtime_versions_stay_distinct() {
        assert_eq!(
            reported_runtime_version(&json!({"runtime_version": "1.14.4-apsp.2"})),
            Some("1.14.4-apsp.2".into())
        );
        assert_eq!(
            reported_runtime_version(&json!({
                "components": {"sing-box": "1.14.4-apsp.2"}
            })),
            None
        );
        assert_eq!(
            embedded_runtime_version(&json!({
                "components": {"sing-box": "1.14.4-apsp.2"}
            })),
            Some("1.14.4-apsp.2".into())
        );
        assert_eq!(
            reported_runtime_version(&json!({"runtime_version": "../bad"})),
            None
        );
        assert_eq!(
            embedded_runtime_version(&json!({"components": {"sing-box": "../bad"}})),
            None
        );
    }
}

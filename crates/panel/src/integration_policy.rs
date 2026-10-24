use apsp_panel_host::{
    AppState,
    error::ApiResult,
    plugin_api::{RoutePolicy, RuntimeStepPolicy, SearchSource},
};
use axum::http::Method;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

pub fn route_policy(path: &str, method: &Method) -> Option<RoutePolicy> {
    let feature = if path.starts_with("/api/plugins/sing-box/") {
        "proxy"
    } else if path.starts_with("/api/plugins/ddns/") {
        "dns"
    } else if path == "/api/plugins/alicloud" || path.starts_with("/api/plugins/alicloud/") {
        "cloud"
    } else {
        return None;
    };
    let mutation = *method != Method::GET && *method != Method::HEAD && *method != Method::OPTIONS;
    let high_risk = mutation
        && ((path.starts_with("/api/plugins/alicloud/")
            && !path.ends_with("/preview")
            && !path.ends_with("/cdt-check")
            && !path.ends_with("/refresh"))
            || (path.starts_with("/api/plugins/sing-box/")
                && !path.ends_with("/preview")
                && (path.contains("/nodes")
                    || path.contains("/accesses")
                    || path.contains("/external-accesses")
                    || path.contains("/subscription/reset"))));
    let read = (feature == "cloud" && *method == Method::POST && path.ends_with("/cdt-check"))
        || feature == "dns"
            && ((path.starts_with("/api/plugins/ddns/rules/") && path.ends_with("/preview"))
                || path.ends_with("/check")
                || path.ends_with("/resolve")
                || (path.starts_with("/api/plugins/ddns/accounts/")
                    && path.contains("/records/")
                    && path.ends_with("/reconcile")));
    Some(RoutePolicy {
        independent_identity: path.starts_with("/api/plugins/sing-box/portal/"),
        feature: Some(feature),
        high_risk,
        read,
        scoped_objects: feature == "dns",
    })
}

pub fn sensitive_key(key: &str) -> bool {
    apsp_plugin_singbox::host_support::sensitive_key(key)
}

pub fn worker_names() -> &'static [&'static str] {
    &[
        "ddns",
        "alicloud",
        "alicloud-cost-cache",
        "alicloud-power",
        "sing-box",
    ]
}

pub fn runtime_step_policy(kind: &str) -> Option<RuntimeStepPolicy> {
    apsp_plugin_singbox::host_support::runtime_step_policy(kind)
}

pub async fn audit_snapshot(state: &AppState, path: &str) -> ApiResult<Option<Value>> {
    apsp_plugin_singbox::host_support::audit_snapshot(state, path).await
}

pub fn signed_tool_packages(entries: &[apsp_panel_host::artifacts::ArtifactEntry]) -> Vec<Value> {
    apsp_plugin_singbox::host_support::signed_tool_packages(entries)
}

pub fn search_sources() -> Vec<SearchSource> {
    apsp_plugin_singbox::host_support::search_sources()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plugin_route_security_keeps_independent_identity_and_typed_read_access() {
        let portal = route_policy("/api/plugins/sing-box/portal/me", &Method::GET).unwrap();
        assert!(portal.independent_identity);
        let reset = route_policy(
            "/api/plugins/sing-box/users/1/subscription/reset",
            &Method::POST,
        )
        .unwrap();
        assert!(reset.high_risk);
        assert!(
            !route_policy("/api/plugins/sing-box/nodes/preview", &Method::POST)
                .unwrap()
                .high_risk
        );
        let dns = route_policy(
            "/api/plugins/ddns/accounts/1/records/2/reconcile",
            &Method::POST,
        )
        .unwrap();
        assert!(dns.read && dns.scoped_objects);
        assert!(
            route_policy("/api/plugins/alicloud/resources/1/power", &Method::POST)
                .unwrap()
                .high_risk
        );
        assert!(
            !route_policy("/api/plugins/alicloud/accounts/1/refresh", &Method::POST)
                .unwrap()
                .high_risk
        );
        let root = route_policy("/api/plugins/alicloud", &Method::PUT).unwrap();
        assert_eq!(root.feature, Some("cloud"));
        assert!(!root.high_risk);
        assert!(
            route_policy("/api/plugins/alicloud/resources/1", &Method::PUT)
                .unwrap()
                .high_risk
        );
        assert!(
            !route_policy("/api/plugins/alicloud/resources/1/preview", &Method::POST)
                .unwrap()
                .high_risk
        );
        assert!(
            !route_policy("/api/plugins/alicloud/resources/1", &Method::GET)
                .unwrap()
                .high_risk
        );
        assert!(route_policy("/api/plugins/unknown/jobs", &Method::POST).is_none());
        let cdt_check =
            route_policy("/api/plugins/alicloud/resources/1/cdt-check", &Method::POST).unwrap();
        assert!(cdt_check.read && !cdt_check.high_risk);
        assert!(
            route_policy("/api/plugins/alicloud/resources/1/cdt-state", &Method::POST)
                .unwrap()
                .high_risk
        );
    }
    #[test]
    fn runtime_step_is_registered_with_its_fixed_version() {
        crate::plugins::install();
        let mut step: apsp_panel_host::operations::Step = serde_json::from_value(json!({
            "kind":"singbox_retry_deployment", "service":null, "timeout_secs":300,
            "runtime_version":"1.14.2"
        }))
        .unwrap();
        assert!(step.validate().is_ok());
        step.runtime_version = Some("latest".into());
        assert!(step.validate().is_err());
        step.runtime_version = Some("1.14.2".into());
        step.service = Some("other.service".into());
        assert!(step.validate().is_err());
    }
}

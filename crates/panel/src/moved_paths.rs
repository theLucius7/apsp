//! Addresses the console API had while it was organised by plugin. A request to one of
//! them is answered at the current address, so pages loaded before an upgrade and
//! operator scripts keep working. Kept for one release after 2026-11-13, then removed —
//! except the expense calendar feed, which calendar programs subscribe to: its former
//! address is an external contract and stays.

use axum::{
    extract::Request,
    http::{Uri, uri::PathAndQuery},
    middleware::Next,
    response::Response,
};

const TRAFFIC: [&str; 11] = [
    "chains",
    "nexus-nodes",
    "node-catalog",
    "nodes",
    "ordered-proxy-resources",
    "ordered-subscription-source-jobs",
    "ordered-subscription-source-previews",
    "ordered-subscription-sources",
    "runtime-inventory",
    "runtime-rollouts",
    "servers",
];

/// The calendar feed's former address and its current one. This pair outlives the
/// console addresses below.
const CALENDAR_FEED: (&str, &str) = (
    "/api/plugins/expenses/calendar.ics",
    "/api/costs/calendar.ics",
);

/// The current path of an address under the former `/api/plugins/` prefix.
fn current_path(path: &str) -> Option<String> {
    if path == CALENDAR_FEED.0 {
        return Some(CALENDAR_FEED.1.into());
    }
    let rest = path.strip_prefix("/api/plugins/")?;
    let (group, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let tail = if tail.is_empty() {
        String::new()
    } else {
        format!("/{tail}")
    };
    Some(match group {
        "sing-box" => {
            let first = tail.trim_start_matches('/').split('/').next()?;
            match first {
                "" => return None,
                "reachability" => format!("/api/observability{tail}"),
                first if TRAFFIC.contains(&first) => format!("/api/traffic{tail}"),
                _ => format!("/api/access{tail}"),
            }
        }
        "ddns" => format!("/api/naming{tail}"),
        "expenses" => format!("/api/costs{tail}"),
        "tcpquality" => format!("/api/observability/tcp-quality{tail}"),
        "alicloud" => format!("/api/resources/alicloud{tail}"),
        _ => return None,
    })
}

/// Rewrites a request for a former address before the gate and the guard see it.
pub async fn current(mut request: Request, next: Next) -> Response {
    if let Some(path) = current_path(request.uri().path()) {
        let target = match request.uri().query() {
            Some(query) => format!("{path}?{query}"),
            None => path,
        };
        if let Ok(target) = target.parse::<PathAndQuery>() {
            let mut parts = request.uri().clone().into_parts();
            parts.path_and_query = Some(target);
            if let Ok(uri) = Uri::from_parts(parts) {
                *request.uri_mut() = uri;
            }
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::current_path;

    #[test]
    fn former_addresses_map_to_the_owning_domain() {
        for (former, now) in [
            ("/api/plugins/sing-box/nodes/7", "/api/traffic/nodes/7"),
            (
                "/api/plugins/sing-box/servers/7/deployments",
                "/api/traffic/servers/7/deployments",
            ),
            ("/api/plugins/sing-box/users", "/api/access/users"),
            ("/api/plugins/sing-box/portal/me", "/api/access/portal/me"),
            ("/api/plugins/sing-box/statistics", "/api/access/statistics"),
            (
                "/api/plugins/sing-box/reachability/settings",
                "/api/observability/reachability/settings",
            ),
            ("/api/plugins/ddns/rules/3", "/api/naming/rules/3"),
            ("/api/plugins/expenses/summary", "/api/costs/summary"),
            (
                "/api/plugins/expenses/calendar.ics",
                "/api/costs/calendar.ics",
            ),
            (
                "/api/plugins/tcpquality/servers/7/targets",
                "/api/observability/tcp-quality/servers/7/targets",
            ),
            ("/api/plugins/alicloud", "/api/resources/alicloud"),
            (
                "/api/plugins/alicloud/resources/1/power-policy",
                "/api/resources/alicloud/resources/1/power-policy",
            ),
        ] {
            assert_eq!(current_path(former).as_deref(), Some(now), "{former}");
        }
    }

    #[test]
    fn other_addresses_are_left_alone() {
        for path in [
            "/api/servers/7",
            "/api/traffic/nodes",
            "/api/plugins/sing-box",
            "/api/plugins/unknown/jobs",
            "/sub/token",
        ] {
            assert_eq!(current_path(path), None, "{path}");
        }
    }
}

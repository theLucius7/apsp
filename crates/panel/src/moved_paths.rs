//! Permanent rewrite of the expense calendar feed's former `/api/plugins/expenses/calendar.ics`
//! address (ADR 0132, ADR 0140).
//!
//! Temporary `/api/plugins/*` rewrites for console and portal routes have been removed after
//! their transition release. Calendar programs subscribe directly to the feed address, so its
//! former URL remains an external contract and is rewritten to `/api/costs/calendar.ics` before
//! the gate and the guard see the request.

use axum::{
    extract::Request,
    http::{Uri, uri::PathAndQuery},
    middleware::Next,
    response::Response,
};

const CALENDAR_FEED: (&str, &str) = (
    "/api/plugins/expenses/calendar.ics",
    "/api/costs/calendar.ics",
);

fn current_path(path: &str) -> Option<&'static str> {
    (path == CALENDAR_FEED.0).then_some(CALENDAR_FEED.1)
}

/// Rewrites a request for a permanent former address before the gate and the guard see it.
pub async fn current(mut request: Request, next: Next) -> Response {
    if let Some(path) = current_path(request.uri().path()) {
        let target = match request.uri().query() {
            Some(query) => format!("{path}?{query}"),
            None => path.to_owned(),
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
    fn calendar_feed_contract_maps_to_costs_domain() {
        assert_eq!(
            current_path("/api/plugins/expenses/calendar.ics"),
            Some("/api/costs/calendar.ics"),
        );
    }

    #[test]
    fn retired_plugin_and_current_addresses_are_not_rewritten() {
        for path in [
            "/api/plugins/sing-box",
            "/api/plugins/sing-box/nodes/7",
            "/api/plugins/sing-box/servers/7/deployments",
            "/api/plugins/sing-box/users",
            "/api/plugins/sing-box/portal/me",
            "/api/plugins/sing-box/statistics",
            "/api/plugins/sing-box/reachability/settings",
            "/api/plugins/ddns/rules/3",
            "/api/plugins/expenses/summary",
            "/api/plugins/expenses/calendar.ics/extra",
            "/api/plugins/tcpquality/servers/7/targets",
            "/api/plugins/alicloud",
            "/api/plugins/alicloud/resources/1/power-policy",
            "/api/plugins/unknown/jobs",
            "/api/servers/7",
            "/api/traffic/nodes",
            "/api/costs/calendar.ics",
            "/sub/token",
        ] {
            assert_eq!(current_path(path), None, "{path}");
        }
    }
}

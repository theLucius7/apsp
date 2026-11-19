#![forbid(unsafe_code)]
//! Only the permanent `/api/plugins/expenses/calendar.ics` external feed contract is rewritten
//! to `/api/costs/calendar.ics`; former `/api/plugins/*` management and portal addresses are
//! retired (`crates/panel/src/moved_paths.rs`).

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test]
async fn calendar_feed_contract_rewrites_while_retired_plugin_routes_do_not(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;

    // Calendar programs keep fetching the feed at the address they subscribed to, with query
    // parameters preserved and the current route's guard enforced before execution.
    let former_feed = panel
        .admin(
            Method::GET,
            "/api/plugins/expenses/calendar.ics?from=2099-02-01&to=2099-03-31",
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .text()
        .await?;
    let current_feed = panel
        .admin(
            Method::GET,
            "/api/costs/calendar.ics?from=2099-02-01&to=2099-03-31",
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert!(former_feed.starts_with("BEGIN:VCALENDAR"));
    assert_eq!(former_feed, current_feed);

    for path in [
        "/api/plugins/expenses/calendar.ics",
        "/api/plugins/expenses/calendar.ics?token=invalid",
        "/api/costs/calendar.ics",
    ] {
        assert_eq!(
            panel.admin(Method::GET, path, "", None).await?.status(),
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }

    // Former management and portal `/api/plugins/*` addresses are no longer rewritten.
    assert_eq!(
        panel
            .admin(
                Method::POST,
                "/api/plugins/sing-box/users",
                &cookie,
                Some(json!({"name": "Retired address"})),
            )
            .await?
            .status(),
        StatusCode::NOT_FOUND,
    );
    for path in [
        "/api/plugins/sing-box/users",
        "/api/plugins/sing-box/nodes",
        "/api/plugins/sing-box/usage?user_id=1",
        "/api/plugins/sing-box/portal/me",
        "/api/plugins/sing-box/reachability/settings",
        "/api/plugins/ddns/rules",
        "/api/plugins/expenses/summary",
        "/api/plugins/tcpquality/servers/1/targets",
        "/api/plugins/alicloud",
        "/api/plugins/unknown/jobs",
    ] {
        assert_eq!(
            panel
                .admin(Method::GET, path, &cookie, None)
                .await?
                .status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    Ok(())
}

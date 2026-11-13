#![forbid(unsafe_code)]
//! Addresses the console API had while it was organised by plugin are answered at their
//! current addresses for one release (`crates/panel/src/moved_paths.rs`).

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

#[sqlx::test]
async fn former_console_addresses_reach_the_current_routes_with_their_guard(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;

    // A write through a former address creates the same object the current address lists.
    let created: Value = panel
        .admin(
            Method::POST,
            "/api/plugins/sing-box/users",
            &cookie,
            Some(json!({"name":"Former address"})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let user = id(&created)?;
    let current: Value = panel
        .admin(Method::GET, "/api/access/users", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    let former: Value = panel
        .admin(Method::GET, "/api/plugins/sing-box/users", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(current, former);
    assert!(
        current
            .as_array()
            .is_some_and(|users| users.iter().any(|entry| entry["id"] == user))
    );

    // The query string travels with the path, and every former group has its place.
    for (former, current) in [
        ("/api/plugins/sing-box/nodes", "/api/traffic/nodes"),
        (
            "/api/plugins/sing-box/usage?user_id=1",
            "/api/access/usage?user_id=1",
        ),
        ("/api/plugins/ddns/rules", "/api/naming/rules"),
        ("/api/plugins/expenses/summary", "/api/costs/summary"),
        ("/api/plugins/alicloud", "/api/resources/alicloud"),
    ] {
        let now = panel.admin(Method::GET, current, &cookie, None).await?;
        let before = panel.admin(Method::GET, former, &cookie, None).await?;
        assert_eq!(before.status(), now.status(), "{former}");
        assert!(now.status().is_success(), "{current}: {}", now.status());
        assert_eq!(
            before.json::<Value>().await?,
            now.json::<Value>().await?,
            "{former}"
        );
    }

    // Calendar programs keep fetching the feed at the address they subscribed to.
    let feed = panel
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
    assert!(feed.starts_with("BEGIN:VCALENDAR"));

    // The guard judges the current address: without a session a former address is refused
    // like the current one, and a path under the former prefix that maps to nothing is
    // not an interface.
    for path in ["/api/plugins/sing-box/users", "/api/access/users"] {
        assert_eq!(
            panel.admin(Method::GET, path, "", None).await?.status(),
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    let unknown = panel
        .admin(Method::GET, "/api/plugins/unknown/jobs", &cookie, None)
        .await?;
    assert!(
        !unknown.status().is_success()
            || !unknown
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("application/json")),
        "an unknown former path is not an API response"
    );
    Ok(())
}

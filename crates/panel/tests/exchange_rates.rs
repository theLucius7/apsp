#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

#[sqlx::test]
async fn exchange_reads_follow_dashboard_access_and_manual_refresh_requires_administration(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    for path in ["/api/exchange-rates", "/api/dashboard/exchange-rates"] {
        assert_eq!(
            panel
                .client
                .get(format!("{}{path}", panel.base))
                .send()
                .await?
                .status(),
            StatusCode::UNAUTHORIZED,
        );
    }
    let response = panel
        .admin(Method::GET, "/api/exchange-rates", &cookie, None)
        .await?
        .error_for_status()?;
    let empty: Value = response.json().await?;
    assert_eq!(empty["status"], "unavailable");
    assert_eq!(empty["rates"], json!({"CNY":1.0}));
    assert!(empty["attempted_at"].is_null());

    sqlx::query("UPDATE panel_settings SET settings=settings || '{\"public_dashboard\":true}'::jsonb WHERE singleton")
        .execute(&panel.state.pool).await?;
    let public = panel
        .client
        .get(format!("{}/api/dashboard/exchange-rates", panel.base))
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(public.headers()["cache-control"], "no-store");
    assert_eq!(public.json::<Value>().await?, empty);
    assert_eq!(
        panel
            .client
            .get(format!("{}/api/exchange-rates", panel.base))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        panel
            .client
            .post(format!("{}/api/exchange-rates/refresh", panel.base))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let attempted: Option<i64> =
        sqlx::query_scalar("SELECT attempted_at FROM exchange_rates WHERE singleton")
            .fetch_one(&panel.state.pool)
            .await?;
    assert!(
        attempted.is_none(),
        "reads and rejected requests must never download"
    );

    // Model an already claimed worker so the real HTTP route exercises its
    // throttle without sending a request to an external exchange provider.
    sqlx::query("UPDATE exchange_rates SET attempted_at=$1,lease_until=$1+60 WHERE singleton")
        .bind(apsp_protocol::now_timestamp())
        .execute(&panel.state.pool)
        .await?;
    assert_eq!(
        panel
            .admin(Method::POST, "/api/exchange-rates/refresh", &cookie, None)
            .await?
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    sqlx::query("UPDATE panel_settings SET settings=settings || '{\"public_dashboard\":false}'::jsonb WHERE singleton")
        .execute(&panel.state.pool).await?;
    assert_eq!(
        panel
            .client
            .get(format!("{}/api/dashboard/exchange-rates", panel.base))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    panel
        .admin(Method::GET, "/api/dashboard/exchange-rates", &cookie, None)
        .await?
        .error_for_status()?;
    Ok(())
}

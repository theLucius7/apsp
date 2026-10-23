#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_panel::server_assets::{AssetSettings, TrafficMode, renew_due};
use apsp_protocol::now_timestamp;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

#[test]
fn asset_validation_preserves_decimal_precision_and_bounds() {
    let asset = AssetSettings {
        region: " jp ".into(),
        group_name: " 主力 ".into(),
        tags: vec![" 线路 ".into(), "线路".into()],
        price: Some("0012.5".into()),
        currency: "usd".into(),
        traffic_limit: u64::MAX.to_string(),
        network_interface: "eth*, !eth1, 以太网 *".into(),
        ..Default::default()
    }
    .normalized()
    .unwrap();
    assert_eq!(asset.region, "JP");
    assert_eq!(asset.tags, vec!["线路"]);
    assert_eq!(asset.price.as_deref(), Some("12.50"));
    assert_eq!(asset.traffic_limit, "18446744073709551615");
    assert!(asset.includes("eth0"));
    assert!(!asset.includes("eth1"));
    assert!(!asset.includes("veth0"));
    assert!(asset.includes("以太网 2"));
    for value in [
        "-1",
        "NaN",
        "1e2",
        "0.001",
        "1000000000.01",
        "999999999999999999999",
    ] {
        assert!(
            AssetSettings {
                price: Some(value.into()),
                ..Default::default()
            }
            .normalized()
            .is_err()
        );
    }
    assert_eq!(
        AssetSettings {
            price: Some("0".into()),
            ..Default::default()
        }
        .normalized()
        .unwrap()
        .price
        .as_deref(),
        Some("0.00")
    );
    assert!(
        AssetSettings::default()
            .normalized()
            .unwrap()
            .price
            .is_none()
    );
    assert!(
        AssetSettings {
            network_interface: "!lo*,!veth*".into(),
            ..Default::default()
        }
        .includes("eth0")
    );
    assert_eq!(
        TrafficMode::Sum.used(u64::MAX.into(), u64::MAX.into()),
        u128::from(u64::MAX) * 2
    );
    assert_eq!(
        [
            TrafficMode::Sum,
            TrafficMode::Max,
            TrafficMode::Min,
            TrafficMode::Up,
            TrafficMode::Down
        ]
        .map(|mode| mode.used(10, 20)),
        [30, 20, 10, 10, 20]
    );
}

#[sqlx::test]
async fn asset_api_is_authenticated_atomic_and_compatible_with_name_only_edits(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let asset = json!({"region":" jp ","group_name":"主力","tags":["线路:BGP","线路:BGP"],
        "hidden":true,"price":"12.5","currency":"usd","billing_cycle":90,
        "expires_at":now_timestamp()+86400,"traffic_limit":"9007199254740993","reset_day":31,
        "network_interface":"eth*,!eth1"});
    let body = json!({"name":"资产测试","asset_settings":asset});
    assert_eq!(
        panel
            .client
            .post(format!("{}/api/servers", panel.base))
            .json(&body)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = panel
        .admin(Method::POST, "/api/servers", &cookie, Some(body))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let server: Value = response.json().await?;
    let id = server["id"].as_i64().unwrap();
    let path = format!("/api/servers/{id}");
    assert_eq!(server["asset_settings"]["region"], "JP");
    assert_eq!(server["asset_settings"]["price"], "12.50");
    assert_eq!(server["asset_settings"]["currency"], "USD");
    assert_eq!(server["asset_settings"]["tags"], json!(["线路:BGP"]));
    assert_eq!(
        server["asset_settings"]["traffic_limit"],
        "9007199254740993"
    );
    let renamed: Value = panel
        .admin(
            Method::PATCH,
            &path,
            &cookie,
            Some(json!({"name":"重命名"})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(renamed["asset_settings"], server["asset_settings"]);
    assert!(renamed["traffic"]["observed_from"].is_null());
    assert!(renamed["traffic"]["percent"].is_null());
    let invalid = [
        json!({"price":"0.001"}),
        json!({"traffic_limit":"18446744073709551616"}),
        json!({"reset_day":0}),
        json!({"reset_day":32}),
        json!({"currency":"bad-code"}),
        json!({"tags":vec!["tag";17]}),
        json!({"tags":[" "]}),
        json!({"group_name":"a".repeat(41)}),
        json!({"region":"bad\ninvalid"}),
        json!({"auto_renewal":true}),
        json!({"network_interface":"!"}),
    ];
    for asset in invalid {
        let response = panel
            .admin(
                Method::PATCH,
                &path,
                &cookie,
                Some(json!({"name":"must-not-save","asset_settings":asset})),
            )
            .await?;
        assert!(response.status().is_client_error());
        let saved: Value = panel
            .admin(Method::GET, &path, &cookie, None)
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(saved["name"], "重命名");
        assert_eq!(saved["asset_settings"], server["asset_settings"]);
    }
    let cleared: Value = panel
        .admin(
            Method::PATCH,
            &path,
            &cookie,
            Some(json!({"name":"清空资产","asset_settings":{}})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(cleared["asset_settings"], json!(AssetSettings::default()));
    let legacy = panel.create_server(&cookie, "旧请求").await?;
    let saved: Value = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{legacy}"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(saved["asset_settings"], json!(AssetSettings::default()));
    let migrated: i64 =
        sqlx::query_scalar("INSERT INTO servers(name) VALUES('migration-fixture') RETURNING id")
            .fetch_one(&panel.state.pool)
            .await?;
    let saved: Value = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{migrated}"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(saved["asset_settings"], json!(AssetSettings::default()));
    Ok(())
}

#[sqlx::test]
async fn renewal_catches_up_without_charging_or_repeating(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let id = panel.create_server(&cookie, "续期").await?;
    let now = now_timestamp();
    let previous = now - 65 * 86400;
    let asset = AssetSettings {
        expires_at: Some(previous),
        auto_renewal: true,
        billing_cycle: 30,
        ..Default::default()
    };
    sqlx::query("UPDATE servers SET asset_settings=$2 WHERE id=$1")
        .bind(id)
        .bind(json!(asset))
        .execute(&panel.state.pool)
        .await?;
    let projected: Value = panel
        .admin(Method::GET, &format!("/api/servers/{id}"), &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        projected["asset_settings"]["expires_at"],
        previous + 90 * 86400
    );
    renew_due(&panel.state.pool, now).await?;
    renew_due(&panel.state.pool, now).await?;
    let persisted: Value = sqlx::query_scalar("SELECT asset_settings FROM servers WHERE id=$1")
        .bind(id)
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(persisted["expires_at"], previous + 90 * 86400);
    let asset = AssetSettings {
        auto_renewal: false,
        ..asset
    };
    sqlx::query("UPDATE servers SET asset_settings=$2 WHERE id=$1")
        .bind(id)
        .bind(json!(asset))
        .execute(&panel.state.pool)
        .await?;
    renew_due(&panel.state.pool, now).await?;
    let persisted: Value = sqlx::query_scalar("SELECT asset_settings FROM servers WHERE id=$1")
        .bind(id)
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(persisted["expires_at"], previous);
    Ok(())
}

#[sqlx::test]
async fn utc_cycles_handle_short_months_leap_days_and_year_boundaries(pool: PgPool) -> Result<()> {
    for (day, reset, expected) in [
        ("2024-02-28", 31, "2024-01-31"),
        ("2024-02-29", 31, "2024-02-29"),
        ("2023-02-28", 31, "2023-02-28"),
        ("2024-03-30", 31, "2024-02-29"),
        ("2024-03-31", 31, "2024-03-31"),
        ("2026-01-01", 15, "2025-12-15"),
        ("2026-01-15", 15, "2026-01-15"),
        ("2026-01-01", 1, "2026-01-01"),
    ] {
        let result: String = sqlx::query_scalar(
            "SELECT (to_timestamp(apsp_traffic_cycle_start(EXTRACT(EPOCH FROM $1::date::timestamp AT TIME ZONE 'UTC')::bigint,$2)) AT TIME ZONE 'UTC')::date::text",
        ).bind(day).bind(reset).fetch_one(&pool).await?;
        assert_eq!(result, expected, "{day}/{reset}");
    }
    Ok(())
}

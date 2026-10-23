#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_protocol::{
    Metrics, NetworkMetrics, TelemetryBatch, TelemetrySample, telemetry::now_millis,
};
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test]
async fn live_receipt_is_not_persistence_and_public_scope_is_rechecked_for_each_snapshot(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (id, _socket, ack) = panel.authenticated_device(&cookie, "live").await?;
    let at = now_millis() - 3000;
    let sample = TelemetrySample {
        id: Uuid::new_v4(),
        sampled_at: at,
        metrics: Metrics {
            cpu_percent: Some(55.0),
            network_interfaces: [(
                "PRIVATE_INTERFACE".into(),
                NetworkMetrics {
                    receive_bytes_per_sec: Some(40.0),
                    ..Default::default()
                },
            )]
            .into(),
            extra: [("PRIVATE_INTERNAL".into(), json!("secret fixture"))].into(),
            ..Default::default()
        },
    };
    let endpoint = format!("{}/api/agent/v1/telemetry/live", panel.base);
    assert_eq!(
        panel
            .client
            .post(&endpoint)
            .json(&sample)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    panel
        .client
        .post(&endpoint)
        .bearer_auth(&ack.session_token)
        .json(&sample)
        .send()
        .await?
        .error_for_status()?;
    let persisted: i64 = sqlx::query_scalar("SELECT metrics_sampled_at FROM servers WHERE id=$1")
        .bind(id)
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(persisted, 0);
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_receipts WHERE server_id=$1")
            .bind(id)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(count, 0);
    let private: Value = panel
        .admin(Method::GET, "/api/dashboard/live", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(private["public_view"], false);
    assert_eq!(private["servers"][0]["metrics_sampled_at"], at);
    assert!(private["servers"][0]["metrics_persisted_at"].is_null());
    let received = private["servers"][0]["metrics_received_at"].clone();
    let mut older = sample.clone();
    older.id = Uuid::new_v4();
    older.sampled_at -= 1000;
    older.metrics.cpu_percent = Some(1.0);
    panel
        .client
        .post(&endpoint)
        .bearer_auth(&ack.session_token)
        .json(&older)
        .send()
        .await?
        .error_for_status()?;
    panel
        .client
        .post(&endpoint)
        .bearer_auth(&ack.session_token)
        .json(&sample)
        .send()
        .await?
        .error_for_status()?;
    let private: Value = panel
        .admin(Method::GET, "/api/dashboard/live", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(private["servers"][0]["metrics_received_at"], received);
    assert_eq!(private["servers"][0]["latest_metrics"]["cpu_percent"], 55.0);
    let public_endpoint = format!("{}/api/dashboard/live", panel.base);
    assert_eq!(
        panel.client.get(&public_endpoint).send().await?.status(),
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE panel_settings SET settings=settings || '{\"public_dashboard\":true}'::jsonb WHERE singleton").execute(&panel.state.pool).await?;
    let response = panel
        .client
        .get(&public_endpoint)
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = response.text().await?;
    assert!(!body.contains("PRIVATE"));
    let public: Value = serde_json::from_str(&body)?;
    assert_eq!(public["public_view"], true);
    assert_eq!(public["servers"][0]["id"], id);
    assert_eq!(
        public["servers"][0]["latest_metrics"]["network_interfaces"]["网卡 1"]["receive_bytes_per_sec"],
        40.0
    );
    // Commit the unchanged sample through the legacy endpoint. A newer live
    // sample would still win the display, while this timestamp marks durability.
    panel
        .client
        .post(format!("{}/api/agent/v1/telemetry", panel.base))
        .bearer_auth(&ack.session_token)
        .json(&TelemetryBatch {
            samples: vec![sample],
        })
        .send()
        .await?
        .error_for_status()?;
    let public: Value = panel
        .client
        .get(&public_endpoint)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(public["servers"][0]["metrics_persisted_at"], at);
    let history = panel
        .client
        .get(format!(
            "{}/api/dashboard/servers/{id}/history?window=24h",
            panel.base
        ))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert!(!history.contains("PRIVATE"));
    assert!(!history.contains("selected_network"));
    sqlx::query("UPDATE servers SET asset_settings=asset_settings || '{\"hidden\":true}'::jsonb WHERE id=$1").bind(id).execute(&panel.state.pool).await?;
    let public: Value = panel
        .client
        .get(&public_endpoint)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(public["servers"].as_array().unwrap().is_empty());
    assert_eq!(
        panel
            .client
            .get(format!("{}/api/dashboard/servers/{id}/history", panel.base))
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE panel_settings SET settings=settings || '{\"public_dashboard\":false}'::jsonb WHERE singleton").execute(&panel.state.pool).await?;
    assert_eq!(
        panel.client.get(&public_endpoint).send().await?.status(),
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[sqlx::test]
async fn capacity_extensions_are_numeric_and_legacy_public_rows_cannot_expose_nested_values(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (id, _socket, ack) = panel
        .authenticated_device(&cookie, "capacity boundary")
        .await?;
    let live = format!("{}/api/agent/v1/telemetry/live", panel.base);
    let durable = format!("{}/api/agent/v1/telemetry", panel.base);
    for key in ["memory_total", "disk_total"] {
        for value in [
            json!({"PRIVATE_CAPACITY":"secret"}),
            json!("PRIVATE_CAPACITY"),
            json!(-1),
            json!(1.5),
        ] {
            let sample = TelemetrySample {
                id: Uuid::new_v4(),
                sampled_at: now_millis(),
                metrics: Metrics {
                    extra: [(key.into(), value)].into(),
                    ..Default::default()
                },
            };
            for (endpoint, body) in [
                (&live, json!(&sample)),
                (
                    &durable,
                    json!(TelemetryBatch {
                        samples: vec![sample.clone()]
                    }),
                ),
            ] {
                assert_eq!(
                    panel
                        .client
                        .post(endpoint)
                        .bearer_auth(&ack.session_token)
                        .json(&body)
                        .send()
                        .await?
                        .status(),
                    StatusCode::BAD_REQUEST
                );
            }
        }
    }
    let receipts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_receipts WHERE server_id=$1")
            .bind(id)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(receipts, 0);
    // Old optional extension fields may already contain arbitrary JSON. Keep
    // that administrator evidence while filtering every anonymous projection.
    sqlx::query("UPDATE servers SET latest_metrics=$2,metrics_sampled_at=$3 WHERE id=$1")
        .bind(id).bind(json!({"cpu_percent":0,"memory_used":0,"memory_total":{"PRIVATE_CAPACITY":"secret"},"disk_total":"PRIVATE_CAPACITY","network_interfaces":{"PRIVATE_INTERFACE":{"received_bytes":0,"receive_bytes_per_sec":{"PRIVATE_RATE":"secret"}}}})).bind(now_millis()).execute(&panel.state.pool).await?;
    sqlx::query("UPDATE panel_settings SET settings=settings || '{\"public_dashboard\":true}'::jsonb WHERE singleton").execute(&panel.state.pool).await?;
    for route in [
        "/api/dashboard/live".to_owned(),
        format!("/api/dashboard/servers/{id}"),
    ] {
        let response = panel
            .client
            .get(format!("{}{route}", panel.base))
            .send()
            .await?
            .error_for_status()?;
        let body = response.text().await?;
        assert!(!body.contains("PRIVATE"), "{route}: {body}");
        let value: Value = serde_json::from_str(&body)?;
        let metrics = if route.ends_with("/live") {
            &value["servers"][0]["latest_metrics"]
        } else {
            &value["latest_metrics"]
        };
        assert_eq!(metrics["cpu_percent"], 0);
        assert_eq!(metrics["memory_used"], 0);
        assert_eq!(metrics["network_interfaces"]["网卡 1"]["received_bytes"], 0);
    }
    let private = panel
        .admin(Method::GET, &format!("/api/servers/{id}"), &cookie, None)
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert!(private.contains("PRIVATE_CAPACITY"));
    Ok(())
}

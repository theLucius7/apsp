#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_protocol::{
    AgentSettings, Metrics, TelemetryAck, TelemetryBatch, TelemetrySample, telemetry::now_millis,
};
use business_support::TestPanel;
use flate2::{Compression, write::GzEncoder};
use reqwest::{Method, StatusCode};
use serde_json::json;
use sqlx::PgPool;
use std::io::Write;
use uuid::Uuid;

fn body(batch: &TelemetryBatch) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&serde_json::to_vec(batch)?)?;
    Ok(encoder.finish()?)
}

#[sqlx::test]
async fn compressed_replays_are_atomic_and_delayed_samples_do_not_replace_current_metrics(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "telemetry").await?;
    let newer = TelemetrySample {
        id: Uuid::new_v4(),
        sampled_at: now_millis(),
        metrics: Metrics {
            cpu_percent: Some(80.0),
            ..Default::default()
        },
    };
    let older = TelemetrySample {
        id: Uuid::new_v4(),
        sampled_at: newer.sampled_at - 1000,
        metrics: Metrics {
            cpu_percent: Some(5.0),
            ..Default::default()
        },
    };
    let batch = TelemetryBatch {
        samples: vec![newer.clone(), older],
    };
    for _ in 0..2 {
        let response = panel
            .client
            .post(format!("{}/api/agent/v1/telemetry", panel.base))
            .bearer_auth(&ack.session_token)
            .header("Content-Encoding", "gzip")
            .body(body(&batch)?)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let received: TelemetryAck = response.json().await?;
        assert_eq!(
            received.ids,
            batch.samples.iter().map(|v| v.id).collect::<Vec<_>>()
        );
    }
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_samples WHERE server_id=$1")
            .bind(server)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(count, 2);
    let current: serde_json::Value =
        sqlx::query_scalar("SELECT latest_metrics FROM servers WHERE id=$1")
            .bind(server)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(current["cpu_percent"], 80.0);
    let mut changed = newer.clone();
    changed.metrics.cpu_percent = Some(99.0);
    let mut first = newer;
    first.id = Uuid::new_v4();
    let invalid = TelemetryBatch {
        samples: vec![first, changed],
    };
    let response = panel
        .client
        .post(format!("{}/api/agent/v1/telemetry", panel.base))
        .bearer_auth(&ack.session_token)
        .json(&invalid)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_samples WHERE server_id=$1")
            .bind(server)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(count, 2);
    let response = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{server}/metrics"),
            &cookie,
            None,
        )
        .await?;
    assert_eq!(response.json::<Vec<TelemetrySample>>().await?.len(), 2);
    Ok(())
}

#[sqlx::test]
async fn settings_require_auth_and_telemetry_rejects_oversized_decompression(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (_server, _socket, ack) = panel.authenticated_device(&cookie, "limits").await?;
    let server = panel.create_server(&cookie, "settings").await?;
    let endpoint = format!("/api/servers/{server}/agent-settings");
    let unauthorized = panel
        .client
        .get(format!("{}{endpoint}", panel.base))
        .send()
        .await?;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let settings = AgentSettings {
        sample_interval_secs: 1,
        upload_interval_secs: 3,
        auto_update: true,
        discover_public_ips: false,
    };
    let response = panel
        .admin(
            Method::PATCH,
            &endpoint,
            &cookie,
            Some(serde_json::to_value(&settings)?),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<AgentSettings>().await?, settings);
    assert_eq!(
        panel
            .admin(
                Method::PATCH,
                &endpoint,
                &cookie,
                Some(json!({"sample_interval_secs":60,"upload_interval_secs":1}))
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&vec![b'x'; 1024 * 1024 + 1])?;
    let response = panel
        .client
        .post(format!("{}/api/agent/v1/telemetry", panel.base))
        .bearer_auth(&ack.session_token)
        .header("Content-Encoding", "gzip")
        .body(encoder.finish()?)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

#[sqlx::test]
async fn heartbeat_and_sample_times_are_distinct_and_stale_history_remains_visible(
    pool: PgPool,
) -> Result<()> {
    use apsp_protocol::{Envelope, Heartbeat, now_timestamp};
    use business_support::send_envelope;
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, mut socket, ack) = panel
        .authenticated_device(&cookie, "cached telemetry")
        .await?;
    let endpoint = format!("/api/servers/{server}");
    let initial: serde_json::Value = panel
        .admin(Method::GET, &endpoint, &cookie, None)
        .await?
        .json()
        .await?;
    assert!(initial["last_heartbeat_at"].is_null());
    assert!(initial["metrics_sampled_at"].is_null());
    let sampled = now_millis() - 30_000;
    let batch = TelemetryBatch {
        samples: vec![TelemetrySample {
            id: Uuid::new_v4(),
            sampled_at: sampled,
            metrics: Metrics {
                cpu_percent: Some(7.5),
                ..Default::default()
            },
        }],
    };
    panel
        .client
        .post(format!("{}/api/agent/v1/telemetry", panel.base))
        .bearer_auth(&ack.session_token)
        .json(&batch)
        .send()
        .await?
        .error_for_status()?;
    send_envelope(
        &mut socket,
        Envelope::new(
            "heartbeat",
            Heartbeat {
                applied: Default::default(),
                uptime_secs: 42,
            },
        )?,
    )
    .await?;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let value: serde_json::Value = panel
                .admin(Method::GET, &endpoint, &cookie, None)
                .await?
                .json()
                .await?;
            if value["last_heartbeat_at"].as_i64().is_some() {
                assert!(value["last_heartbeat_at"].as_i64().unwrap() >= now_timestamp() - 2);
                assert_eq!(value["metrics_sampled_at"], sampled);
                assert_eq!(value["latest_metrics"]["cpu_percent"], 7.5);
                assert_eq!(value["online"], true);
                assert_eq!(value["metrics_stale"], true);
                break Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    // Slow, explicitly configured sampling must not be reported as expired early.
    panel
        .admin(
            Method::PATCH,
            &format!("{endpoint}/agent-settings"),
            &cookie,
            Some(json!({"sample_interval_secs":60,"upload_interval_secs":60})),
        )
        .await?
        .error_for_status()?;
    let slow: serde_json::Value = panel
        .admin(Method::GET, &endpoint, &cookie, None)
        .await?
        .json()
        .await?;
    assert_eq!(slow["metrics_stale"], false);
    assert_eq!(slow["metrics_sampled_at"], sampled);
    Ok(())
}

#[sqlx::test]
async fn legacy_metrics_keep_values_without_inventing_collection_time(pool: PgPool) -> Result<()> {
    use apsp_protocol::Envelope;
    use business_support::send_envelope;
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, mut socket, _) = panel
        .authenticated_device(&cookie, "legacy telemetry")
        .await?;
    send_envelope(
        &mut socket,
        Envelope::new(
            "telemetry.metrics",
            Metrics {
                cpu_percent: Some(17.5),
                ..Default::default()
            },
        )?,
    )
    .await?;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let value: serde_json::Value = panel
                .admin(
                    Method::GET,
                    &format!("/api/servers/{server}"),
                    &cookie,
                    None,
                )
                .await?
                .json()
                .await?;
            if value["latest_metrics"]["cpu_percent"] == 17.5 {
                assert!(value["metrics_sampled_at"].is_null());
                assert!(value["last_heartbeat_at"].is_null());
                break Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    Ok(())
}

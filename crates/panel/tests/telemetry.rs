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
        sqlx::query_scalar("SELECT latest_metrics FROM telemetry_servers WHERE server_id=$1")
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

/// The top-level keys of a JSON object body, in the order the body lists them.
fn top_level_keys(body: &str) -> Vec<String> {
    let (mut keys, mut depth, mut chars) = (Vec::new(), 0, body.chars().peekable());
    while let Some(c) = chars.next() {
        match c {
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            '"' => {
                let mut text = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        c => text.push(c),
                    }
                }
                if depth == 1 && chars.peek() == Some(&':') {
                    keys.push(text);
                }
            }
            _ => {}
        }
    }
    keys
}

#[sqlx::test]
async fn server_views_show_the_metrics_and_settings_observability_keeps(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let created = panel
        .admin(
            Method::POST,
            "/api/servers",
            &cookie,
            Some(json!({"name":"stored view","telemetry_settings":{"persist_interval_secs":120}})),
        )
        .await?
        .error_for_status()?
        .text()
        .await?;
    // The server view keeps its fields and their order now that observability keeps the
    // metrics and settings in its own table.
    assert_eq!(
        top_level_keys(&created),
        [
            "id",
            "name",
            "device_public_key",
            "static_info",
            "last_seen",
            "last_heartbeat_at",
            "metrics_sampled_at",
            "metrics_received_at",
            "metrics_persisted_at",
            "served_at",
            "telemetry_settings",
            "agent_settings",
            "asset_settings",
            "traffic",
            "latest_metrics",
            "manifest_rev",
            "capabilities",
            "online",
            "metrics_stale",
        ]
    );
    let created: serde_json::Value = serde_json::from_str(&created)?;
    let id = created["id"].as_i64().unwrap();
    assert_eq!(created["latest_metrics"], json!({}));
    assert!(created["metrics_sampled_at"].is_null());
    assert!(created["metrics_persisted_at"].is_null());
    assert_eq!(
        created["telemetry_settings"],
        json!({"persist_interval_secs":120})
    );
    assert_eq!(created["metrics_stale"], true);
    let sampled = now_millis() - 2_000;
    sqlx::query("UPDATE telemetry_servers SET latest_metrics=$2,sampled_at=$3 WHERE server_id=$1")
        .bind(id)
        .bind(json!({"cpu_percent":42.5,"process_resources":{"TEST_ONLY":1},"system_pressure":{"cpu":{"some_avg10":1.5}}}))
        .bind(sampled)
        .execute(&panel.state.pool)
        .await?;
    // A server whose row has no metrics or settings stored shows the defaults.
    let bare: i64 =
        sqlx::query_scalar("INSERT INTO servers(name) VALUES('TEST_ONLY bare') RETURNING id")
            .fetch_one(&panel.state.pool)
            .await?;
    let listed: Vec<serde_json::Value> = panel
        .admin(Method::GET, "/api/servers", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    let detail: serde_json::Value = panel
        .admin(Method::GET, &format!("/api/servers/{id}"), &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    for view in [&listed[0], &detail] {
        assert_eq!(view["id"], id);
        assert_eq!(view["latest_metrics"], json!({"cpu_percent":42.5}));
        assert_eq!(view["metrics_sampled_at"], sampled);
        assert_eq!(view["metrics_persisted_at"], sampled);
        assert!(view["metrics_received_at"].is_null());
        assert_eq!(view["metrics_stale"], false);
        assert_eq!(
            view["telemetry_settings"],
            json!({"persist_interval_secs":120})
        );
    }
    assert_eq!(listed[1]["id"], bare);
    assert_eq!(listed[1]["latest_metrics"], json!({}));
    assert!(listed[1]["metrics_sampled_at"].is_null());
    assert_eq!(
        listed[1]["telemetry_settings"],
        json!({"persist_interval_secs":60})
    );
    // The fleet profile shows the stored process and pressure detail.
    let profile: serde_json::Value = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{id}/fleet"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(profile["sampled_at"], sampled);
    assert_eq!(profile["processes"], json!({"TEST_ONLY":1}));
    assert_eq!(profile["pressure"], json!({"cpu":{"some_avg10":1.5}}));
    let profile: serde_json::Value = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{bare}/fleet"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(profile["sampled_at"], 0);
    assert!(profile["processes"].is_null());
    Ok(())
}

#[sqlx::test]
async fn a_batch_neither_rewrites_nor_waits_for_the_server_row(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "row lock").await?;
    // A contact time update in progress holds the server row, as the device channel does
    // several times a second across the fleet.
    let mut contact = panel.state.pool.begin().await?;
    sqlx::query("UPDATE servers SET last_contact_at=last_contact_at WHERE id=$1")
        .bind(server)
        .execute(&mut *contact)
        .await?;
    let sampled = now_millis() - 1_000;
    let batch = TelemetryBatch {
        samples: vec![TelemetrySample {
            id: Uuid::new_v4(),
            sampled_at: sampled,
            metrics: Metrics {
                cpu_percent: Some(33.0),
                ..Default::default()
            },
        }],
    };
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        panel
            .client
            .post(format!("{}/api/agent/v1/telemetry", panel.base))
            .bearer_auth(&ack.session_token)
            .header("Content-Encoding", "gzip")
            .body(body(&batch)?)
            .send(),
    )
    .await
    .expect("a telemetry batch waited for the server row")?;
    assert_eq!(response.status(), StatusCode::OK);
    contact.rollback().await?;
    let stored: (i64, serde_json::Value) = sqlx::query_as(
        "SELECT sampled_at,latest_metrics FROM telemetry_servers WHERE server_id=$1",
    )
    .bind(server)
    .fetch_one(&panel.state.pool)
    .await?;
    assert_eq!(stored.0, sampled);
    assert_eq!(stored.1["cpu_percent"], 33.0);
    // A deleted server takes no more batches.
    sqlx::query("UPDATE servers SET deleted_at=1 WHERE id=$1")
        .bind(server)
        .execute(&panel.state.pool)
        .await?;
    let response = panel
        .client
        .post(format!("{}/api/agent/v1/telemetry", panel.base))
        .bearer_auth(&ack.session_token)
        .header("Content-Encoding", "gzip")
        .body(body(&batch)?)
        .send()
        .await?;
    assert_ne!(response.status(), StatusCode::OK);
    Ok(())
}

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
use std::time::Duration;
use uuid::Uuid;

fn sample(at: i64, cpu: Option<f64>, bytes: u64) -> TelemetrySample {
    TelemetrySample {
        id: Uuid::new_v4(),
        sampled_at: at,
        metrics: Metrics {
            cpu_percent: cpu,
            uptime_secs: Some((at / 1000) as u64),
            network_interfaces: [(
                "fixture0".into(),
                NetworkMetrics {
                    received_bytes: Some(bytes),
                    transmitted_bytes: Some(bytes),
                    receive_bytes_per_sec: Some(12.0),
                    transmit_bytes_per_sec: Some(15.0),
                },
            )]
            .into(),
            ..Default::default()
        },
    }
}

async fn post(
    panel: &TestPanel,
    token: &str,
    samples: Vec<TelemetrySample>,
) -> Result<reqwest::Response> {
    Ok(panel
        .client
        .post(format!("{}/api/agent/v1/telemetry", panel.base))
        .bearer_auth(token)
        .json(&TelemetryBatch { samples })
        .send()
        .await?)
}

#[sqlx::test]
async fn replay_identity_outlives_raw_rows_and_aggregates_only_observed_values(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "tier replay").await?;
    let at = (now_millis() - 3 * 3_600_000).div_euclid(60_000) * 60_000;
    let first = sample(at + 1000, Some(10.0), u64::MAX - 1000);
    let second = sample(at + 2000, None, u64::MAX - 900);
    let third = sample(at + 61_000, Some(90.0), u64::MAX - 800);
    post(
        &panel,
        &ack.session_token,
        vec![second.clone(), first.clone()],
    )
    .await?
    .error_for_status()?;
    post(&panel, &ack.session_token, vec![third.clone()])
        .await?
        .error_for_status()?;
    let before:Value=sqlx::query_scalar("SELECT jsonb_build_object('up',SUM(uploaded)::text,'down',SUM(downloaded)::text) FROM server_network_daily WHERE server_id=$1").bind(server).fetch_one(&panel.state.pool).await?;
    assert_eq!(before, json!({"up":"200","down":"200"}));
    apsp_panel::telemetry::maintain(&panel.state.pool).await?;
    for _ in 0..2 {
        post(
            &panel,
            &ack.session_token,
            vec![first.clone(), third.clone()],
        )
        .await?
        .error_for_status()?;
    }
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_samples WHERE server_id=$1")
            .bind(server)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(count, 0);
    let points =
        apsp_panel::telemetry::read_window(&panel.state.pool, server, at, at + 120_000, 300_000)
            .await?;
    assert_eq!(
        points.iter().map(|point| point.sample_count).sum::<u64>(),
        3
    );
    let cpu_count: u64 = points
        .iter()
        .filter_map(|point| point.metrics.get("cpu_percent"))
        .map(|metric| metric.count)
        .sum();
    assert_eq!(cpu_count, 2);
    assert!(
        points
            .iter()
            .all(|point| !point.metrics.contains_key("memory_percent"))
    );
    let saved: Value = sqlx::query_scalar(
        "SELECT summary FROM telemetry_history WHERE server_id=$1 ORDER BY bucket_at DESC LIMIT 1",
    )
    .bind(server)
    .fetch_one(&panel.state.pool)
    .await?;
    assert_eq!(
        saved["network_counters"]["fixture0"]["received_bytes"],
        (u64::MAX - 800).to_string()
    );
    let mut changed = first.clone();
    changed.metrics.cpu_percent = Some(20.0);
    let new = sample(at + 62_000, Some(99.0), u64::MAX - 500);
    assert_eq!(
        post(&panel, &ack.session_token, vec![new, changed])
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let after:Value=sqlx::query_scalar("SELECT jsonb_build_object('up',SUM(uploaded)::text,'down',SUM(downloaded)::text) FROM server_network_daily WHERE server_id=$1").bind(server).fetch_one(&panel.state.pool).await?;
    assert_eq!(before, after);
    let receipts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_receipts WHERE server_id=$1")
            .bind(server)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(receipts, 3);
    Ok(())
}

fn point(at: i64, count: u64, average: f64, minimum: f64, maximum: f64) -> Value {
    json!({"bucket_at":at,"sample_count":count,"first_sampled_at":at+1000,"last_sampled_at":at+20_000,"metrics":{"cpu_percent":{"count":count,"avg":average,"min":minimum,"max":maximum}},"network_counters":{},"partial":false})
}

#[sqlx::test]
async fn rollups_are_atomic_weighted_and_repeatable_and_retention_runs_without_ingest(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "rollup").await?;
    panel
        .admin(
            Method::PATCH,
            "/api/telemetry/policy",
            &cookie,
            Some(json!({"history_retention_days":365})),
        )
        .await?
        .error_for_status()?;
    sqlx::query("INSERT INTO telemetry_history_initialized(server_id) VALUES($1)")
        .bind(server)
        .execute(&panel.state.pool)
        .await?;
    let now = now_millis();
    let week = (now - 8 * 86_400_000).div_euclid(300_000) * 300_000;
    let month = (now - 35 * 86_400_000).div_euclid(3_600_000) * 3_600_000;
    for at in [week, month] {
        for (offset, count, avg, min, max) in
            [(0, 2, 20.0, 10.0, 30.0), (60_000, 3, 70.0, 40.0, 90.0)]
        {
            sqlx::query("INSERT INTO telemetry_history(server_id,resolution_secs,bucket_at,summary) VALUES($1,60,$2,$3)").bind(server).bind(at+offset).bind(point(at+offset,count,avg,min,max)).execute(&panel.state.pool).await?;
        }
    }
    for _ in 0..3 {
        apsp_panel::telemetry::maintain_at(&panel.state.pool, now, Duration::from_secs(6)).await?;
    }
    let rows:Vec<(i32,Value)>=sqlx::query_as("SELECT resolution_secs,summary FROM telemetry_history WHERE server_id=$1 ORDER BY bucket_at").bind(server).fetch_all(&panel.state.pool).await?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, 3600);
    assert_eq!(rows[1].0, 300);
    for (_, value) in rows {
        assert_eq!(value["sample_count"], 5);
        assert_eq!(
            value["metrics"]["cpu_percent"],
            json!({"count":5,"avg":50.0,"min":10.0,"max":90.0})
        );
    }
    panel
        .admin(
            Method::PATCH,
            "/api/telemetry/policy",
            &cookie,
            Some(json!({"history_retention_days":1})),
        )
        .await?
        .error_for_status()?;
    apsp_panel::telemetry::maintain_at(&panel.state.pool, now, Duration::from_secs(6)).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_history WHERE server_id=$1")
            .bind(server)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(count, 0);
    Ok(())
}

#[sqlx::test]
async fn bootstrap_preserves_uninitialized_raw_and_busy_servers_do_not_block_maintenance(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let mut ids = Vec::new();
    let at = now_millis() - 3 * 3_600_000;
    for index in 0..10 {
        let id = panel
            .create_server(&cookie, &format!("legacy {index}"))
            .await?;
        ids.push(id);
        let sample = sample(at, Some(30.0), 100);
        sqlx::query("INSERT INTO telemetry_samples(server_id,id,sampled_at,digest,metrics) VALUES($1,$2,$3,'TEST_ONLY',$4)").bind(id).bind(sample.id).bind(at).bind(json!(sample.metrics)).execute(&panel.state.pool).await?;
    }
    apsp_panel::telemetry::maintain_at(&panel.state.pool, now_millis(), Duration::from_secs(6))
        .await?;
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_samples WHERE server_id=$1")
            .bind(ids[9])
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(remaining, 1);
    let mut tx = panel.state.pool.begin().await?;
    sqlx::query("SELECT id FROM servers WHERE id=$1 FOR UPDATE")
        .bind(ids[8])
        .fetch_one(&mut *tx)
        .await?;
    tokio::time::timeout(
        Duration::from_secs(1),
        apsp_panel::telemetry::maintain_at(
            &panel.state.pool,
            now_millis(),
            Duration::from_millis(150),
        ),
    )
    .await??;
    tx.rollback().await?;
    apsp_panel::telemetry::maintain_at(&panel.state.pool, now_millis(), Duration::from_secs(6))
        .await?;
    let summaries: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_history WHERE server_id=ANY($1)")
            .bind(&ids)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(summaries, 10);
    let partial:bool=sqlx::query_scalar("SELECT BOOL_AND((summary->>'partial')::boolean) FROM telemetry_history WHERE server_id=ANY($1)").bind(&ids).fetch_one(&panel.state.pool).await?;
    assert!(partial);
    Ok(())
}

#[sqlx::test]
async fn independent_telemetry_settings_validate_create_and_keep_legacy_settings_strict(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    for interval in [0, 14, 3601] {
        assert_eq!(panel.admin(Method::POST,"/api/servers",&cookie,Some(json!({"name":"invalid schedule","telemetry_settings":{"persist_interval_secs":interval}}))).await?.status(),StatusCode::BAD_REQUEST);
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM servers")
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(count, 0);
    let server: Value = panel
        .admin(
            Method::POST,
            "/api/servers",
            &cookie,
            Some(json!({"name":"schedule","telemetry_settings":{"persist_interval_secs":120}})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = server["id"].as_i64().unwrap();
    assert_eq!(server["telemetry_settings"]["persist_interval_secs"], 120);
    let settings: Value = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{id}/telemetry-settings"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(settings, json!({"persist_interval_secs":120}));
    let old = panel.create_server(&cookie, "default schedule").await?;
    let default: Value = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{old}/telemetry-settings"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(default["persist_interval_secs"], 60);
    let legacy: apsp_protocol::AgentSettings = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{id}/agent-settings"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(legacy.valid());
    for days in [0, 3651] {
        assert_eq!(
            panel
                .admin(
                    Method::PATCH,
                    "/api/telemetry/policy",
                    &cookie,
                    Some(json!({"history_retention_days":days}))
                )
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    Ok(())
}

#[sqlx::test]
async fn locked_bootstrap_prefix_does_not_starve_other_servers(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let at = now_millis() - 3 * 3_600_000;
    let mut ids = Vec::new();
    for index in 0..10 {
        let id = panel
            .create_server(&cookie, &format!("bootstrap queue {index}"))
            .await?;
        let sample = sample(at, Some(30.0), 100);
        sqlx::query("INSERT INTO telemetry_samples(server_id,id,sampled_at,digest,metrics) VALUES($1,$2,$3,'TEST_ONLY',$4)").bind(id).bind(sample.id).bind(at).bind(json!(sample.metrics)).execute(&panel.state.pool).await?;
        ids.push(id);
    }
    let mut locked = panel.state.pool.begin().await?;
    sqlx::query("SELECT id FROM servers WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(&ids[..8])
        .fetch_all(&mut *locked)
        .await?;
    apsp_panel::telemetry::maintain_at(&panel.state.pool, now_millis(), Duration::from_secs(6))
        .await?;
    let initialized: Vec<i64> = sqlx::query_scalar(
        "SELECT server_id FROM telemetry_history_initialized ORDER BY server_id",
    )
    .fetch_all(&panel.state.pool)
    .await?;
    assert_eq!(initialized, ids[8..].to_vec());
    let summaries: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_history WHERE server_id=ANY($1)")
            .bind(&ids[8..])
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(summaries, 2);
    let retained: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM telemetry_samples WHERE server_id=ANY($1)")
            .bind(&ids[..8])
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(retained, 8);
    locked.rollback().await?;
    Ok(())
}

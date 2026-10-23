#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_panel::{server_traffic, servers::Server};
use apsp_protocol::{
    Metrics, NetworkMetrics, TelemetryAck, TelemetryBatch, TelemetrySample, telemetry::now_millis,
};
use business_support::TestPanel;
use reqwest::{Method, Response, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

fn sample(at: i64, received: Option<u64>, transmitted: Option<u64>) -> TelemetrySample {
    TelemetrySample {
        id: Uuid::new_v4(),
        sampled_at: at,
        metrics: Metrics {
            network_interfaces: [(
                "eth0".into(),
                NetworkMetrics {
                    received_bytes: received,
                    transmitted_bytes: transmitted,
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        },
    }
}

async fn send(panel: &TestPanel, token: &str, samples: Vec<TelemetrySample>) -> Result<Response> {
    Ok(panel
        .client
        .post(format!("{}/api/agent/v1/telemetry", panel.base))
        .bearer_auth(token)
        .json(&TelemetryBatch { samples })
        .send()
        .await?)
}

async fn totals(pool: &PgPool, server: i64) -> Result<(String, String)> {
    Ok(sqlx::query_as("SELECT COALESCE(SUM(uploaded),0)::text,COALESCE(SUM(downloaded),0)::text FROM server_network_daily WHERE server_id=$1")
        .bind(server).fetch_one(pool).await?)
}

async fn summary(pool: &PgPool, server: i64, at: i64) -> Result<Value> {
    let mut rows: Vec<Server> = sqlx::query_as("SELECT * FROM servers WHERE id=$1")
        .bind(server)
        .fetch_all(pool)
        .await?;
    server_traffic::attach(pool, &mut rows, at / 1000).await?;
    Ok(serde_json::to_value(&rows[0].traffic)?)
}

#[sqlx::test]
async fn telemetry_accumulates_in_time_order_and_replays_or_conflicts_do_not_change_totals(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "计量").await?;
    let t = now_millis() - 10_000;
    let first = sample(t, Some(1000), Some(2000));
    let second = sample(t + 1000, Some(1100), Some(2200));
    let third = sample(t + 2000, Some(1250), Some(2300));
    let batch = vec![third.clone(), first, second];
    for _ in 0..2 {
        let response = send(&panel, &ack.session_token, batch.clone())
            .await?
            .error_for_status()?;
        assert_eq!(
            response.json::<TelemetryAck>().await?.ids,
            batch.iter().map(|s| s.id).collect::<Vec<_>>()
        );
        assert_eq!(
            totals(&panel.state.pool, server).await?,
            ("300".into(), "250".into())
        );
    }
    send(
        &panel,
        &ack.session_token,
        vec![sample(t - 1000, Some(900), Some(1900))],
    )
    .await?
    .error_for_status()?;
    assert_eq!(
        totals(&panel.state.pool, server).await?,
        ("300".into(), "250".into())
    );
    let mut changed = third;
    changed
        .metrics
        .network_interfaces
        .get_mut("eth0")
        .unwrap()
        .received_bytes = Some(9);
    let rejected = sample(t + 3000, Some(1500), Some(2600));
    assert_eq!(
        send(&panel, &ack.session_token, vec![rejected, changed])
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        totals(&panel.state.pool, server).await?,
        ("300".into(), "250".into())
    );
    send(
        &panel,
        &ack.session_token,
        vec![sample(t + 4000, Some(10), Some(20))],
    )
    .await?
    .error_for_status()?;
    assert_eq!(
        totals(&panel.state.pool, server).await?,
        ("320".into(), "260".into())
    );
    let actual = summary(&panel.state.pool, server, t + 4000).await?;
    assert_eq!(actual["incomplete"], true);
    assert_eq!(actual["observed_from"], t);
    assert_eq!(actual["used"], "580");
    Ok(())
}

#[sqlx::test]
async fn missing_counters_and_new_interfaces_establish_baselines_and_large_values_remain_exact(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "大整数").await?;
    let t = now_millis() - 10_000;
    send(&panel, &ack.session_token, vec![sample(t, None, Some(500))])
        .await?
        .error_for_status()?;
    assert!(summary(&panel.state.pool, server, t).await?["observed_from"].is_null());
    send(
        &panel,
        &ack.session_token,
        vec![sample(t + 1000, Some(0), Some(0))],
    )
    .await?
    .error_for_status()?;
    send(
        &panel,
        &ack.session_token,
        vec![sample(t + 2000, Some(u64::MAX), Some(u64::MAX))],
    )
    .await?
    .error_for_status()?;
    let actual = summary(&panel.state.pool, server, t + 2000).await?;
    assert_eq!(actual["used"], "36893488147419103230");
    assert_eq!(actual["uploaded"], u64::MAX.to_string());
    assert_eq!(actual["incomplete"], true);
    let mut added = sample(t + 3000, Some(u64::MAX), Some(u64::MAX));
    added.metrics.network_interfaces.insert(
        "eth1".into(),
        NetworkMetrics {
            received_bytes: Some(500),
            transmitted_bytes: Some(500),
            ..Default::default()
        },
    );
    send(&panel, &ack.session_token, vec![added])
        .await?
        .error_for_status()?;
    assert_eq!(
        totals(&panel.state.pool, server).await?,
        (u64::MAX.to_string(), u64::MAX.to_string())
    );
    let response: Value = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{server}"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(response["traffic"]["uploaded"].is_string());
    Ok(())
}

#[sqlx::test]
async fn cycle_and_interface_edits_reaggregate_daily_history_without_resetting_it(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let id = panel.create_server(&cookie, "跨月").await?;
    let at: i64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM TIMESTAMPTZ '2024-02-28 23:59:00+00')::bigint*1000",
    )
    .fetch_one(&panel.state.pool)
    .await?;
    let mut samples = [
        sample(at, Some(100), Some(200)),
        sample(at + 60_000, Some(150), Some(260)),
        sample(at + 86_460_000, Some(200), Some(320)),
    ];
    let mut tx = panel.state.pool.begin().await?;
    sqlx::query("SELECT id FROM servers WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    server_traffic::ingest(&mut tx, id, &mut samples).await?;
    tx.commit().await?;
    let asset = json!({"reset_day":31,"traffic_limit":"50","traffic_limit_type":"up","network_interface":"eth*,!eth1"});
    sqlx::query("UPDATE servers SET asset_settings=$2 WHERE id=$1")
        .bind(id)
        .bind(asset)
        .execute(&panel.state.pool)
        .await?;
    let value = summary(&panel.state.pool, id, at + 86_460_000).await?;
    assert_eq!(value["uploaded"], "120");
    assert_eq!(value["downloaded"], "100");
    assert_eq!(value["used"], "120");
    assert_eq!(value["remaining"], "0");
    assert_eq!(value["exceeded"], true);
    let end: i64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM TIMESTAMPTZ '2024-03-31 00:00:00+00')::bigint",
    )
    .fetch_one(&panel.state.pool)
    .await?;
    assert_eq!(value["cycle_end"], end);
    sqlx::query("UPDATE servers SET asset_settings=$2 WHERE id=$1")
        .bind(id)
        .bind(json!({"reset_day":1,"network_interface":"eth0"}))
        .execute(&panel.state.pool)
        .await?;
    let value = summary(&panel.state.pool, id, at + 86_460_000).await?;
    assert_eq!(value["used"], "110");
    sqlx::query("UPDATE servers SET asset_settings=$2 WHERE id=$1")
        .bind(id)
        .bind(json!({"network_interface":"!eth*"}))
        .execute(&panel.state.pool)
        .await?;
    let value = summary(&panel.state.pool, id, at + 86_460_000).await?;
    assert!(value["observed_from"].is_null());
    assert!(value["percent"].is_null());
    assert_eq!(
        totals(&panel.state.pool, id).await?,
        ("120".into(), "100".into())
    );
    Ok(())
}

#[sqlx::test]
async fn clock_jump_and_storage_failure_keep_checkpoint_telemetry_and_daily_rows_consistent(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "故障恢复").await?;
    let at = now_millis() - 4_000_000;
    let mut first = sample(at, Some(1000), Some(2000));
    first.metrics.uptime_secs = Some(100);
    send(&panel, &ack.session_token, vec![first])
        .await?
        .error_for_status()?;
    sqlx::raw_sql("CREATE FUNCTION reject_daily_fixture() RETURNS TRIGGER AS $$ BEGIN RAISE EXCEPTION 'TEST_ONLY'; END; $$ LANGUAGE plpgsql;
        CREATE TRIGGER reject_daily_fixture BEFORE INSERT ON server_network_daily FOR EACH ROW EXECUTE FUNCTION reject_daily_fixture();")
        .execute(&panel.state.pool).await?;
    let mut next = sample(at + 3_600_000, Some(1010), Some(2020));
    next.metrics.uptime_secs = Some(110);
    assert_eq!(
        send(&panel, &ack.session_token, vec![next.clone()])
            .await?
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        totals(&panel.state.pool, server).await?,
        ("0".into(), "0".into())
    );
    let saved: (i64,i64) = sqlx::query_as("SELECT s.metrics_sampled_at,(c.checkpoint->>'sampled_at')::bigint FROM servers s JOIN server_network_counters c ON c.server_id=s.id WHERE s.id=$1").bind(server).fetch_one(&panel.state.pool).await?;
    assert_eq!(saved, (at, at));
    sqlx::query("DROP TRIGGER reject_daily_fixture ON server_network_daily")
        .execute(&panel.state.pool)
        .await?;
    send(&panel, &ack.session_token, vec![next])
        .await?
        .error_for_status()?;
    assert_eq!(
        totals(&panel.state.pool, server).await?,
        ("20".into(), "10".into())
    );
    assert_eq!(
        summary(&panel.state.pool, server, at + 3_600_000).await?["incomplete"],
        true
    );
    Ok(())
}

#[sqlx::test]
async fn uptime_regression_counts_new_counters_even_when_they_increase(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "明确重启").await?;
    let at = now_millis() - 10_000;
    let mut first = sample(at, Some(1000), Some(2000));
    first.metrics.uptime_secs = Some(100);
    let mut next = sample(at + 1000, Some(3000), Some(5000));
    next.metrics.uptime_secs = Some(10);
    send(&panel, &ack.session_token, vec![first, next.clone()])
        .await?
        .error_for_status()?;
    send(&panel, &ack.session_token, vec![next])
        .await?
        .error_for_status()?;
    assert_eq!(
        totals(&panel.state.pool, server).await?,
        ("5000".into(), "3000".into())
    );
    assert_eq!(
        summary(&panel.state.pool, server, at + 1000).await?["incomplete"],
        true
    );
    Ok(())
}

#[sqlx::test]
async fn clamped_wall_clock_keeps_monotonic_counter_deltas(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "时钟回拨").await?;
    let at = now_millis() - 10_000;
    let mut first = sample(at, Some(1000), Some(2000));
    first.metrics.uptime_secs = Some(100);
    let mut next = sample(at + 1, Some(1010), Some(2020));
    next.metrics.uptime_secs = Some(110);
    send(&panel, &ack.session_token, vec![first, next])
        .await?
        .error_for_status()?;
    assert_eq!(
        totals(&panel.state.pool, server).await?,
        ("20".into(), "10".into())
    );
    assert_eq!(
        summary(&panel.state.pool, server, at + 1).await?["incomplete"],
        true
    );
    Ok(())
}

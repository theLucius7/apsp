#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
use anyhow::Result;
use apsp_panel::{notifications, settings::Settings};
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

async fn config(pool: &PgPool, expiry: u16, traffic: u8) -> Result<()> {
    let settings = Settings {
        offline_alerts: false,
        expiry_alert_days: expiry,
        traffic_alert_percentage: traffic,
        telegram_enabled: true,
        telegram_token: "123:TEST_ONLY_SECRET_00000000000".into(),
        telegram_chat_id: "-100000".into(),
        ..Default::default()
    };
    sqlx::query("UPDATE panel_settings SET settings=$1")
        .bind(json!(settings))
        .execute(pool)
        .await?;
    Ok(())
}

async fn count(pool: &PgPool, category: &str) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM server_alert_events WHERE category=$1")
            .bind(category)
            .fetch_one(pool)
            .await?,
    )
}

fn spec(aggregation: &str, ids: Vec<i64>) -> Value {
    json!({"name":"负载规则","metric":"cpu","threshold":90,"duration_minutes":2,"aggregation":aggregation,"all_servers":false,"enabled":true,"server_ids":ids})
}

async fn minute(pool: &PgPool, server: i64, bucket: i64, cpu: f64) -> Result<()> {
    let at = bucket * 1000;
    let summary = json!({"bucket_at":at,"sample_count":2,"first_sampled_at":at+1000,"last_sampled_at":at+59000,
        "metrics":{"cpu_percent":{"count":2,"avg":cpu,"min":cpu,"max":cpu}},"network_counters":{},"partial":false});
    sqlx::query("INSERT INTO telemetry_history(server_id,resolution_secs,bucket_at,summary) VALUES($1,60,$2,$3) ON CONFLICT(server_id,resolution_secs,bucket_at) DO UPDATE SET summary=$3")
        .bind(server).bind(at).bind(summary).execute(pool).await?;
    sqlx::query(
        "INSERT INTO telemetry_history_initialized(server_id) VALUES($1) ON CONFLICT DO NOTHING",
    )
    .bind(server)
    .execute(pool)
    .await?;
    Ok(())
}

#[sqlx::test]
async fn resource_windows_require_complete_valid_samples_and_recover_once(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let pool = &panel.state.pool;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "resource fixture").await?;
    let excluded = panel.create_server(&cookie, "excluded").await?;
    let now = apsp_protocol::now_timestamp();
    let end = now / 60 * 60;
    config(pool, 0, 0).await?;
    sqlx::query("UPDATE servers SET last_seen=$1,metrics_sampled_at=$1*1000")
        .bind(now)
        .execute(pool)
        .await?;
    for id in [server, excluded] {
        for (offset, cpu) in [(120, 100), (60, 85)] {
            sqlx::query("INSERT INTO metrics_minutely(server_id,bucket,metrics,sampled_at) VALUES($1,$2,$3,$4)")
                .bind(id).bind(end-offset).bind(json!({"cpu_percent":cpu})).bind((end-offset)*1000+1000).execute(pool).await?;
        }
    }
    let mut rules = Vec::new();
    for aggregation in ["average", "continuous"] {
        let rule: Value = panel
            .admin(
                Method::POST,
                "/api/alert-rules",
                &cookie,
                Some(json!({"spec":spec(aggregation,vec![server])})),
            )
            .await?
            .error_for_status()?
            .json()
            .await?;
        rules.push(rule);
    }
    notifications::evaluate(pool, now, now).await?;
    assert_eq!(
        count(pool, "resource").await?,
        0,
        "legacy last observations are not complete aggregates"
    );
    for id in [server, excluded] {
        for (offset, cpu) in [(120, 100.0), (60, 85.0)] {
            minute(pool, id, end - offset, cpu).await?;
        }
    }
    let (a, b) = tokio::join!(
        notifications::evaluate(pool, now, now),
        notifications::evaluate(pool, now, now)
    );
    a?;
    b?;
    assert_eq!(
        count(pool, "resource").await?,
        1,
        "average exceeds threshold; the minimum does not"
    );
    let (owner, value): (i64, Value) =
        sqlx::query_as("SELECT server_id,details FROM server_alert_events")
            .fetch_one(pool)
            .await?;
    assert_eq!(owner, server);
    assert_eq!(value["value"], 92.5);
    sqlx::query("UPDATE telemetry_history SET summary=jsonb_set(summary,'{metrics}','{}') WHERE server_id=$1 AND bucket_at=$2")
        .bind(server)
        .bind((end - 60) * 1000)
        .execute(pool)
        .await?;
    notifications::evaluate(pool, now, now).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM server_alert_events WHERE resolved_at IS NULL"
        )
        .fetch_one(pool)
        .await?,
        1,
        "missing is not recovery"
    );
    for offset in [120, 60] {
        minute(pool, server, end - offset, 10.0).await?;
    }
    notifications::evaluate(pool, now, now).await?;
    notifications::evaluate(pool, now, now).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notification_outbox")
            .fetch_one(pool)
            .await?,
        2,
        "one alert plus one recovery"
    );
    // A gap must not be hidden by an out-of-window sample.
    let recovery: String =
        sqlx::query_scalar("SELECT message FROM notification_outbox WHERE kind='recovery'")
            .fetch_one(pool)
            .await?;
    assert!(
        recovery.contains("10.00"),
        "recovery reports the observed value"
    );
    for offset in [120, 60] {
        minute(pool, server, end - offset, 100.0).await?;
    }
    sqlx::query("DELETE FROM telemetry_history WHERE server_id=$1 AND bucket_at=$2")
        .bind(server)
        .bind((end - 60) * 1000)
        .execute(pool)
        .await?;
    notifications::evaluate(pool, now, now).await?;
    assert_eq!(count(pool, "resource").await?, 1);
    let path = format!("/api/alert-rules/{}", rules[0]["id"].as_str().unwrap());
    let mut updated = spec("average", vec![server]);
    updated["enabled"] = json!(false);
    let body = json!({"spec":updated,"revision":1});
    panel
        .admin(Method::PATCH, &path, &cookie, Some(body.clone()))
        .await?
        .error_for_status()?;
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(body))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    panel
        .admin(Method::DELETE, &path, &cookie, None)
        .await?
        .error_for_status()?;
    assert_eq!(
        count(pool, "resource").await?,
        1,
        "rule deletion preserves history"
    );
    sqlx::query("UPDATE servers SET deleted_at=$2 WHERE id=$1")
        .bind(server)
        .bind(now)
        .execute(pool)
        .await?;
    let remaining: Vec<Value> = panel
        .admin(Method::GET, "/api/alert-rules", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(remaining[0]["spec"]["server_ids"], json!([]));
    let mut paused = remaining[0]["spec"].clone();
    paused["enabled"] = json!(false);
    let path = format!("/api/alert-rules/{}", remaining[0]["id"].as_str().unwrap());
    panel
        .admin(
            Method::PATCH,
            &path,
            &cookie,
            Some(json!({"spec":paused,"revision":remaining[0]["revision"]})),
        )
        .await?
        .error_for_status()?;
    Ok(())
}

#[sqlx::test]
async fn expiry_and_traffic_reminders_deduplicate_by_record_and_cycle(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let pool = &panel.state.pool;
    let cookie = panel.admin_cookie().await?;
    let id = panel.create_server(&cookie, "asset fixture").await?;
    let now = apsp_protocol::now_timestamp();
    config(pool, 7, 80).await?;
    sqlx::query("UPDATE servers SET asset_settings=$2 WHERE id=$1")
        .bind(id)
        .bind(json!({"expires_at":now+2*86400,"traffic_limit":"1000"}))
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO server_network_daily(server_id,day,interface,uploaded,downloaded,first_sample_at,last_sample_at) VALUES($1,$2,'eth0',850,0,$3,$3)")
        .bind(id).bind(now/86400*86400).bind(now*1000).execute(pool).await?;
    notifications::evaluate(pool, now, now).await?;
    notifications::evaluate(pool, now, now).await?;
    assert_eq!(count(pool, "expiry").await?, 1);
    assert_eq!(count(pool, "traffic").await?, 2, "80% and 85% milestones");
    let cycle: i64 = sqlx::query_scalar("SELECT apsp_traffic_cycle_start($1,1)")
        .bind(now)
        .fetch_one(pool)
        .await?;
    sqlx::query("INSERT INTO server_traffic_corrections(server_id,cycle_start,reset_day,network_interface,uploaded_offset,downloaded_offset,reason,created_at) VALUES($1,$2,1,'',-400,0,'test', $3)")
        .bind(id).bind(cycle).bind(now).execute(pool).await?;
    notifications::evaluate(pool, now, now).await?;
    sqlx::query("UPDATE server_traffic_corrections SET uploaded_offset=0 WHERE server_id=$1")
        .bind(id)
        .execute(pool)
        .await?;
    notifications::evaluate(pool, now, now).await?;
    assert_eq!(
        count(pool, "traffic").await?,
        2,
        "correction and rising usage must not resend past milestones"
    );
    sqlx::query("UPDATE server_network_daily SET uploaded=1000 WHERE server_id=$1")
        .bind(id)
        .execute(pool)
        .await?;
    notifications::evaluate(pool, now, now).await?;
    assert_eq!(count(pool, "traffic").await?, 5);
    sqlx::query(
        "UPDATE panel_settings SET settings=jsonb_set(settings,'{notification_enabled}','false')",
    )
    .execute(pool)
    .await?;
    notifications::evaluate(pool, now, now).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM notification_outbox WHERE status='pending'"
        )
        .fetch_one(pool)
        .await?,
        0
    );
    config(pool, 7, 80).await?;
    notifications::evaluate(pool, now, now).await?;
    assert_eq!(count(pool, "expiry").await?, 1);
    assert_eq!(count(pool, "traffic").await?, 5);
    // A new billing cycle has independent receipts and observed bytes.
    let next: i64 = sqlx::query_scalar("SELECT apsp_traffic_cycle_start($1+32*86400,1)")
        .bind(cycle)
        .fetch_one(pool)
        .await?;
    sqlx::query("INSERT INTO server_network_daily(server_id,day,interface,uploaded,downloaded,first_sample_at,last_sample_at) VALUES($1,$2,'eth0',800,0,$3,$3)")
        .bind(id).bind(next).bind(next*1000).execute(pool).await?;
    notifications::evaluate(pool, now, next + 60).await?;
    assert_eq!(count(pool, "traffic").await?, 6);
    Ok(())
}

#[sqlx::test]
async fn settings_validate_templates_mask_secrets_and_cancel_changed_recipient(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let pool = &panel.state.pool;
    let cookie = panel.admin_cookie().await?;
    for path in [
        "/api/alert-rules",
        "/api/notifications",
        "/api/latency-tasks",
    ] {
        assert_eq!(
            panel
                .client
                .get(format!("{}{path}", panel.base))
                .send()
                .await?
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        panel
            .client
            .post(format!("{}/api/notifications/telegram/test", panel.base))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        panel
            .admin(
                Method::POST,
                "/api/notifications/telegram/test",
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let mut settings = json!({"public_dashboard":false,"offline_alerts":true,"offline_minutes":2,"telegram_enabled":true,
        "telegram_token":"123:TEST_ONLY_SECRET_00000000000","telegram_chat_id":"-100000","telegram_thread_id":12,
        "telegram_template":"{{title}} {{server}} {{message}} {{time}} {{event}}","expiry_alert_days":7,"traffic_alert_percentage":80});
    let text = panel
        .admin(
            Method::PATCH,
            "/api/settings",
            &cookie,
            Some(settings.clone()),
        )
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert!(!text.contains("SECRET"));
    assert!(text.contains("telegram_token_configured"));
    settings.as_object_mut().unwrap().remove("telegram_token");
    let id = panel.create_server(&cookie, "secret fixture").await?;
    let now = apsp_protocol::now_timestamp();
    sqlx::query("UPDATE servers SET last_seen=$2 WHERE id=$1")
        .bind(id)
        .bind(now - 600)
        .execute(pool)
        .await?;
    notifications::evaluate(pool, now - 600, now).await?;
    settings["telegram_thread_id"] = json!(13);
    panel
        .admin(
            Method::PATCH,
            "/api/settings",
            &cookie,
            Some(settings.clone()),
        )
        .await?
        .error_for_status()?;
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM notification_outbox")
            .fetch_one(pool)
            .await?,
        "cancelled"
    );
    for invalid in ["{{unknown}}", "{{title", ""] {
        let mut invalid_settings = settings.clone();
        invalid_settings["telegram_template"] = json!(invalid);
        assert_eq!(
            panel
                .admin(
                    Method::PATCH,
                    "/api/settings",
                    &cookie,
                    Some(invalid_settings)
                )
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    settings["telegram_thread_id"] = json!(0);
    let cleared: Value = panel
        .admin(Method::PATCH, "/api/settings", &cookie, Some(settings))
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(cleared["telegram_thread_id"].is_null());
    // Test the request limiter without sending any real Telegram messages.
    sqlx::query("UPDATE notification_test_limit SET sent_at=$1")
        .bind(now)
        .execute(pool)
        .await?;
    assert_eq!(
        panel
            .admin(
                Method::POST,
                "/api/notifications/telegram/test",
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    Ok(())
}

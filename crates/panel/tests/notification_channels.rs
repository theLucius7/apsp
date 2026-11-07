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
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn receiver() -> Result<(String, tokio::task::JoinHandle<Value>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/TEST_ONLY_WEBHOOK_SECRET", listener.local_addr()?);
    Ok((
        url,
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let payload = loop {
                let mut buffer = [0; 4096];
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0 && request.len() + count < 64 * 1024);
                request.extend_from_slice(&buffer[..count]);
                if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    assert!(headers.contains("authorization: bearer test_only_header_secret"));
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap();
                    if request.len() >= end + 4 + length {
                        break serde_json::from_slice(&request[end + 4..end + 4 + length]).unwrap();
                    }
                }
            };
            stream
                .write_all(
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            payload
        }),
    ))
}

fn update(url: &str) -> Value {
    json!({"enabled":true,"preset":"custom","url":url,"headers":"Authorization: Bearer TEST_ONLY_HEADER_SECRET","body":"{\"message\":\"{{message}}\",\"event\":\"{{event}}\",\"key\":\"TEST_ONLY_BODY_SECRET\"}"})
}

#[sqlx::test]
async fn webhook_settings_require_admin_hide_secrets_preserve_blanks_and_test_only_saved_config(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    for (method, path) in [
        (Method::GET, "/api/notifications/channels"),
        (Method::GET, "/api/notifications/webhook"),
        (Method::PATCH, "/api/notifications/webhook"),
        (Method::DELETE, "/api/notifications/webhook"),
        (Method::POST, "/api/notifications/webhook/test"),
    ] {
        let response = panel
            .client
            .request(method.clone(), format!("{}{path}", panel.base))
            .json(&update("https://example.invalid/notify"))
            .send()
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
    }
    let (url, receiver) = receiver().await?;
    for body in [
        update(&url),
        json!({"enabled":false,"preset":"custom","url":"","headers":"","body":""}),
    ] {
        let text = panel
            .admin(
                Method::PATCH,
                "/api/notifications/webhook",
                &cookie,
                Some(body),
            )
            .await?
            .error_for_status()?
            .text()
            .await?;
        assert!(!text.contains("TEST_ONLY"));
        assert!(text.contains("url_configured"));
    }
    let saved = apsp_panel::settings::read(&panel.state.pool)
        .await?
        .webhook
        .unwrap();
    assert_eq!(saved.url, url);
    assert!(!saved.enabled);
    assert!(saved.headers.contains("TEST_ONLY_HEADER_SECRET"));
    // A saved but disabled channel can be tested before enabling automatic delivery.
    panel
        .admin(
            Method::POST,
            "/api/notifications/webhook/test",
            &cookie,
            None,
        )
        .await?
        .error_for_status()?;
    let payload = receiver.await?;
    assert_eq!(payload["event"], "test");
    assert_eq!(payload["key"], "TEST_ONLY_BODY_SECRET");
    assert_eq!(
        panel
            .admin(
                Method::POST,
                "/api/notifications/webhook/test",
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let channels: Value = panel
        .admin(Method::GET, "/api/notifications/channels", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(channels[1]["test"]["success"], true);
    assert!(channels[0]["test"].is_null());
    assert!(!channels.to_string().contains("TEST_ONLY"));
    // Switching presets cannot silently reuse a credential-bearing endpoint or body.
    assert_eq!(
        panel
            .admin(
                Method::PATCH,
                "/api/notifications/webhook",
                &cookie,
                Some(json!({"enabled":true,"preset":"slack"}))
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let general: Value = panel
        .admin(Method::GET, "/api/settings", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(!general.to_string().contains("TEST_ONLY"));
    panel
        .admin(
            Method::PATCH,
            "/api/notifications/webhook",
            &cookie,
            Some(json!({"enabled":false,"preset":"custom","clear_headers":true})),
        )
        .await?
        .error_for_status()?;
    assert!(
        apsp_panel::settings::read(&panel.state.pool)
            .await?
            .webhook
            .unwrap()
            .headers
            .is_empty()
    );
    panel
        .admin(Method::DELETE, "/api/notifications/webhook", &cookie, None)
        .await?
        .error_for_status()?;
    assert!(
        apsp_panel::settings::read(&panel.state.pool)
            .await?
            .webhook
            .is_none()
    );
    Ok(())
}

#[sqlx::test]
async fn changing_one_channel_only_cancels_its_pending_messages(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let pool = &panel.state.pool;
    let cookie = panel.admin_cookie().await?;
    let settings = Settings {
        telegram_enabled: true,
        telegram_token: "123:TEST_ONLY_SECRET_00000000000".into(),
        telegram_chat_id: "-100000".into(),
        ..Default::default()
    };
    sqlx::query("UPDATE panel_settings SET settings=$1")
        .bind(json!(settings))
        .execute(pool)
        .await?;
    panel
        .admin(
            Method::PATCH,
            "/api/notifications/webhook",
            &cookie,
            Some(update("https://example.invalid/notify")),
        )
        .await?
        .error_for_status()?;
    let id = panel
        .create_server(&cookie, "channel settings fixture")
        .await?;
    let now = apsp_protocol::now_timestamp();
    sqlx::query("UPDATE servers SET last_seen=$2 WHERE id=$1")
        .bind(id)
        .bind(now - 600)
        .execute(pool)
        .await?;
    notifications::evaluate(pool, now - 600, now).await?;
    let mut general = json!({"public_dashboard":false,"offline_alerts":true,"offline_minutes":5,"telegram_enabled":true,"telegram_chat_id":"-100001"});
    panel
        .admin(
            Method::PATCH,
            "/api/settings",
            &cookie,
            Some(general.clone()),
        )
        .await?
        .error_for_status()?;
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT channel,status FROM notification_outbox ORDER BY channel")
            .fetch_all(pool)
            .await?;
    assert_eq!(
        rows,
        vec![
            ("telegram".into(), "cancelled".into()),
            ("webhook".into(), "pending".into())
        ]
    );
    assert!(apsp_panel::settings::read(pool).await?.webhook.is_some());
    // Change the Webhook destination, retaining the other channel's new event.
    sqlx::query("UPDATE notification_outbox SET status='pending' WHERE channel='telegram'")
        .execute(pool)
        .await?;
    panel
        .admin(
            Method::PATCH,
            "/api/notifications/webhook",
            &cookie,
            Some(json!({"enabled":true,"preset":"custom","url":"https://example.invalid/new"})),
        )
        .await?
        .error_for_status()?;
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT channel,status FROM notification_outbox ORDER BY channel")
            .fetch_all(pool)
            .await?;
    assert_eq!(
        rows,
        vec![
            ("telegram".into(), "pending".into()),
            ("webhook".into(), "cancelled".into())
        ]
    );
    general["notification_enabled"] = json!(false);
    panel
        .admin(Method::PATCH, "/api/settings", &cookie, Some(general))
        .await?
        .error_for_status()?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM notification_outbox WHERE status='pending'"
        )
        .fetch_one(pool)
        .await?,
        0
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn channel_migration_preserves_old_telegram_messages(pool: PgPool) -> Result<()> {
    for migration in sqlx::migrate!()
        .iter()
        .filter(|migration| migration.version < 31)
    {
        sqlx::raw_sql(&migration.sql).execute(&pool).await?;
    }
    let server: i64 = sqlx::query_scalar(
        "INSERT INTO servers(name) VALUES('notification migration fixture') RETURNING id",
    )
    .fetch_one(&pool)
    .await?;
    let event: i64 = sqlx::query_scalar("INSERT INTO server_alert_events(server_id,server_name,opened_at) VALUES($1,'fixture',1) RETURNING id").bind(server).fetch_one(&pool).await?;
    sqlx::query("INSERT INTO notification_outbox(event_id,kind,message,attempts,next_attempt_at) VALUES($1,'offline','original',3,100)").bind(event).execute(&pool).await?;
    sqlx::raw_sql(include_str!(
        "../migrations/0031_notification_alignment.sql"
    ))
    .execute(&pool)
    .await?;
    let row: (String, String, i32, i64) =
        sqlx::query_as("SELECT channel,message,attempts,next_attempt_at FROM notification_outbox")
            .fetch_one(&pool)
            .await?;
    assert_eq!(row, ("telegram".into(), "original".into(), 3, 100));
    sqlx::query("INSERT INTO notification_outbox(event_id,kind,channel,message,next_attempt_at) VALUES($1,'offline','webhook','{}',100)").bind(event).execute(&pool).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notification_outbox")
            .fetch_one(&pool)
            .await?,
        2
    );
    Ok(())
}

#[sqlx::test]
async fn automatic_webhook_delivers_offline_and_recovery_with_event_metadata(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let pool = &panel.state.pool;
    let server = panel
        .create_server(&cookie, "automatic webhook fixture")
        .await?;
    let now = apsp_protocol::now_timestamp();
    for (recovery, expected) in [(false, "offline"), (true, "online")] {
        let (url, worker) = receiver().await?;
        panel
            .admin(
                Method::PATCH,
                "/api/notifications/webhook",
                &cookie,
                Some(update(&url)),
            )
            .await?
            .error_for_status()?;
        sqlx::query("UPDATE servers SET last_seen=$2 WHERE id=$1")
            .bind(server)
            .bind(if recovery { now } else { now - 600 })
            .execute(pool)
            .await?;
        notifications::evaluate(pool, now - 600, now).await?;
        notifications::dispatch(pool, now).await?;
        let received = tokio::time::timeout(std::time::Duration::from_secs(5), worker).await??;
        assert_eq!(received["event"], expected);
        if recovery {
            assert!(received["message"].as_str().unwrap().contains("恢复在线"));
        }
    }
    let events: Value = panel
        .admin(Method::GET, "/api/notifications", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(events[0]["deliveries"].as_array().unwrap().len(), 2);
    for delivery in events[0]["deliveries"].as_array().unwrap() {
        assert_eq!(delivery["channel"], "webhook");
        assert_eq!(delivery["status"], "sent");
        assert_eq!(delivery["attempts"], 1);
        let attempted = delivery["last_attempt_at"].as_i64().unwrap();
        let delivered = delivery["delivered_at"].as_i64().unwrap();
        assert!(attempted >= now);
        assert!(delivered >= attempted);
    }
    assert!(!events.to_string().contains("TEST_ONLY"));
    Ok(())
}

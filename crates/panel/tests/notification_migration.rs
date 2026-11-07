#![forbid(unsafe_code)]
use anyhow::Result;
use serde_json::{Value, json};
use sqlx::PgPool;

#[sqlx::test(migrations = false)]
async fn extending_alerts_preserves_old_events_settings_and_pending_deliveries(
    pool: PgPool,
) -> Result<()> {
    let migrations = sqlx::migrate!();
    for migration in migrations
        .iter()
        .filter(|migration| migration.version <= 19)
    {
        sqlx::raw_sql(&migration.sql).execute(&pool).await?;
    }
    let server: i64 =
        sqlx::query_scalar("INSERT INTO servers(name) VALUES('upgrade fixture') RETURNING id")
            .fetch_one(&pool)
            .await?;
    let event: i64 = sqlx::query_scalar("INSERT INTO server_offline_events(server_id,server_name,last_seen,opened_at) VALUES($1,'upgrade fixture',10,20) RETURNING id").bind(server).fetch_one(&pool).await?;
    sqlx::query("INSERT INTO notification_outbox(event_id,kind,message,next_attempt_at,attempts) VALUES($1,'offline','original message',30,2)").bind(event).execute(&pool).await?;
    sqlx::query("UPDATE panel_settings SET settings=$1")
        .bind(json!({"public_dashboard":true,"offline_alerts":false,"telegram_enabled":false}))
        .execute(&pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0020_notification_rules.sql"))
        .execute(&pool)
        .await?;
    let migrated: (i64, String, String, i64, i64) = sqlx::query_as(
        "SELECT id,category,source_key,last_seen,opened_at FROM server_alert_events",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        migrated,
        (event, "offline".into(), "offline".into(), 10, 20)
    );
    let delivery: (i64, String, i32, i64) =
        sqlx::query_as("SELECT event_id,message,attempts,next_attempt_at FROM notification_outbox")
            .fetch_one(&pool)
            .await?;
    assert_eq!(delivery, (event, "original message".into(), 2, 30));
    let value: Value = sqlx::query_scalar("SELECT settings FROM panel_settings")
        .fetch_one(&pool)
        .await?;
    let settings: apsp_panel::settings::Settings = serde_json::from_value(value)?;
    assert!(settings.public_dashboard);
    assert!(!settings.offline_alerts);
    assert_eq!(settings.expiry_alert_days, 0);
    assert_eq!(settings.traffic_alert_percentage, 0);
    assert!(settings.telegram_template.contains("{{event}}"));
    Ok(())
}

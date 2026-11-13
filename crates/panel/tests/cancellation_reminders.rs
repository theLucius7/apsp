#![forbid(unsafe_code)]
//! Cancellation reminders (costs) raise incidents (observability) through the platform's
//! evidence interface that the application connects.

use apsp_costs::cancellation_reminders;
use apsp_panel_host::config::Config;
use apsp_protocol::now_timestamp;
use sqlx::PgPool;
use uuid::Uuid;

struct Directory(std::path::PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[sqlx::test]
async fn explicit_reminders_do_not_replay_and_revocation_stops_new_events(
    pool: PgPool,
) -> anyhow::Result<()> {
    apsp_panel::domains::install();
    let directory = Directory(std::env::temp_dir().join(format!(
        "apsp-cancellation-reminder-test-{}",
        Uuid::new_v4()
    )));
    std::fs::create_dir(&directory.0)?;
    let state = apsp_panel::state(
        pool.clone(),
        Config {
            database_url: String::new(),
            listen: "127.0.0.1:0".parse()?,
            public_url: "http://127.0.0.1".into(),
            data_dir: directory.0.clone(),
            admin_password: Some("cancellation-reminder-test-password".into()),
        },
    )
    .await?;
    let server: i64 =
        sqlx::query_scalar("INSERT INTO servers(name) VALUES('TEST_ONLY reminder') RETURNING id")
            .fetch_one(&pool)
            .await?;
    let now = now_timestamp();
    let record = Uuid::new_v4();
    let unconfigured = Uuid::new_v4();
    for id in [record, unconfigured] {
        sqlx::query("INSERT INTO fleet_cost_records(id,server_id,kind,amount_minor,currency,occurred_at,valid_until,reference,created_at) VALUES($1,$2,'cancellation',0,'USD',$3,$4,'TEST_ONLY planned cancellation',$3)").bind(id).bind(server).bind(now).bind(now+3600).execute(&pool).await?;
    }
    sqlx::query("INSERT INTO operations_cancellation_reminders(record_id,enabled,lead_secs,configured_by,configured_at) VALUES($1,true,604800,1,$2)").bind(record).bind(now).execute(&pool).await?;
    cancellation_reminders::tick(&state).await?;
    cancellation_reminders::tick(&state).await?;
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM operations_incidents WHERE source_key LIKE 'cancellation:%'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(events, 1);
    sqlx::query(
        "UPDATE operations_incidents SET status='resolved',resolved_at=$1 WHERE source_key=$2",
    )
    .bind(now)
    .bind(format!("cancellation:{record}"))
    .execute(&pool)
    .await?;
    sqlx::query("UPDATE operations_cancellation_reminders SET enabled=false WHERE record_id=$1")
        .bind(record)
        .execute(&pool)
        .await?;
    cancellation_reminders::tick(&state).await?;
    sqlx::query("UPDATE operations_cancellation_reminders SET enabled=true WHERE record_id=$1")
        .bind(record)
        .execute(&pool)
        .await?;
    cancellation_reminders::tick(&state).await?;
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM operations_incidents WHERE source_key LIKE 'cancellation:%'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(events, 1);
    sqlx::query("INSERT INTO operations_cancellation_reminders(record_id,enabled,lead_secs,configured_by,configured_at) VALUES($1,true,604800,1,$2)").bind(unconfigured).bind(now).execute(&pool).await?;
    sqlx::query("UPDATE administrator_profiles SET enabled=false WHERE admin_id=1")
        .execute(&pool)
        .await?;
    cancellation_reminders::tick(&state).await?;
    let active: bool = sqlx::query_scalar(
        "SELECT enabled FROM operations_cancellation_reminders WHERE record_id=$1",
    )
    .bind(unconfigured)
    .fetch_one(&pool)
    .await?;
    assert!(!active);
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM operations_incidents WHERE source_key LIKE 'cancellation:%'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(events, 1);
    Ok(())
}

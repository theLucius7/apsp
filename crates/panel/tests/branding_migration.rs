#![forbid(unsafe_code)]

use anyhow::Result;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = false)]
async fn branding_upgrade_preserves_cycles_and_managed_listener_uniqueness(
    pool: PgPool,
) -> Result<()> {
    let migrations = sqlx::migrate!("./migrations");
    for migration in migrations.iter().filter(|migration| migration.version < 65) {
        sqlx::raw_sql(&migration.sql).execute(&pool).await?;
    }
    let before: i64 = sqlx::query_scalar("SELECT sinan_traffic_cycle_start(1728000000, 31)")
        .fetch_one(&pool)
        .await?;
    let id = Uuid::new_v4();
    let config = json!({"owner":"sinan","enabled":true,"server_id":1,
        "listen_address":"127.0.0.1","listen_port":18080,"protocol":"tcp"});
    sqlx::query("INSERT INTO network_documents(id,kind,config,created_at,updated_at) VALUES($1,'forwarding',$2,1,1)")
        .bind(id).bind(config).execute(&pool).await?;
    sqlx::raw_sql(include_str!("../migrations/0065_apsp_branding.sql"))
        .execute(&pool)
        .await?;
    let after: i64 = sqlx::query_scalar("SELECT apsp_traffic_cycle_start(1728000000, 31)")
        .fetch_one(&pool)
        .await?;
    assert_eq!(before, after);
    let owner: String =
        sqlx::query_scalar("SELECT config->>'owner' FROM network_documents WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(owner, "apsp");
    let default: String = sqlx::query_scalar("SELECT column_default FROM information_schema.columns WHERE table_schema='public' AND table_name='singbox_subscription_sources' AND column_name='user_agent'")
        .fetch_one(&pool).await?;
    assert!(default.contains("Apsp-subscription-import/1"));
    let duplicate = sqlx::query("INSERT INTO network_documents(id,kind,config,created_at,updated_at) SELECT $1,kind,config,1,1 FROM network_documents WHERE id=$2")
        .bind(Uuid::new_v4()).bind(id).execute(&pool).await.unwrap_err();
    assert!(duplicate.as_database_error().unwrap().is_unique_violation());
    Ok(())
}

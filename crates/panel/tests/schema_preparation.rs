#![forbid(unsafe_code)]
use anyhow::Result;
use sqlx::PgPool;

// A database that ran the source migration between 0062 and 0065 keeps numbered
// sources as a trigger-guarded archive. Upgrading it must not abort on 0065.
#[sqlx::test(migrations = false)]
async fn archived_numbered_sources_do_not_block_the_rename_migration(pool: PgPool) -> Result<()> {
    let mut earlier = sqlx::migrate!("./migrations");
    earlier.migrations = earlier
        .migrations
        .iter()
        .filter(|migration| migration.version <= 64)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    earlier.run(&pool).await?;
    let source: i64 = sqlx::query_scalar("INSERT INTO singbox_subscription_sources(name,kind,secret_content,created_at,user_agent) VALUES('archived','inline','TEST_ONLY content',0,'Sinan-subscription-import/1') RETURNING id")
        .fetch_one(&pool)
        .await?;
    sqlx::query("UPDATE singbox_source_migration SET migrated_at=1")
        .execute(&pool)
        .await?;
    // Without preparation the released 0065 hits the archive trigger.
    assert!(sqlx::migrate!("./migrations").run(&pool).await.is_err());
    apsp_panel::schema::prepare(&pool).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    let agent: String =
        sqlx::query_scalar("SELECT user_agent FROM singbox_subscription_sources WHERE id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await?;
    assert_eq!(agent, "Apsp-subscription-import/1");
    // The archive guard is active again.
    assert!(
        sqlx::query("UPDATE singbox_subscription_sources SET name='changed' WHERE id=$1")
            .bind(source)
            .execute(&pool)
            .await
            .is_err()
    );
    Ok(())
}

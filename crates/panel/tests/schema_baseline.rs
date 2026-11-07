#![forbid(unsafe_code)]

use anyhow::Result;
use apsp_panel::schema::initialize;
use sqlx::PgPool;

async fn rejected(pool: &PgPool) -> Result<()> {
    let error = initialize(pool).await.unwrap_err();
    assert!(format!("{error:#}").contains("手动重建"));
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn concurrent_initialization_and_reopening_preserve_records(pool: PgPool) -> Result<()> {
    let (a, b) = tokio::join!(initialize(&pool), initialize(&pool));
    a?;
    b?;
    let id: i64 =
        sqlx::query_scalar("INSERT INTO servers(name) VALUES('TEST_ONLY baseline') RETURNING id")
            .fetch_one(&pool)
            .await?;
    initialize(&pool).await?;
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT name FROM servers WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await?,
        "TEST_ONLY baseline"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM record_retention_policy")
            .fetch_one(&pool)
            .await?,
        6
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn unknown_database_is_rejected_without_creating_a_ledger(pool: PgPool) -> Result<()> {
    sqlx::raw_sql(
        "CREATE TABLE keep_me(value text); INSERT INTO keep_me VALUES('TEST_ONLY retained')",
    )
    .execute(&pool)
    .await?;
    rejected(&pool).await?;
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT value FROM keep_me")
            .fetch_one(&pool)
            .await?,
        "TEST_ONLY retained"
    );
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
            .fetch_one(&pool)
            .await?
    );
    Ok(())
}

#[sqlx::test]
async fn historical_checksums_and_additional_history_are_rejected(pool: PgPool) -> Result<()> {
    sqlx::query("UPDATE _sqlx_migrations SET checksum=decode('00','hex')")
        .execute(&pool)
        .await?;
    rejected(&pool).await?;
    assert_eq!(
        sqlx::query_scalar::<_, Vec<u8>>("SELECT checksum FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await?,
        vec![0]
    );
    sqlx::query("UPDATE _sqlx_migrations SET checksum=$1")
        .bind(sqlx::migrate!().iter().next().unwrap().checksum.as_ref())
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(91,'historical',true,decode('00','hex'),0)").execute(&pool).await?;
    rejected(&pool).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await?,
        2
    );
    Ok(())
}

#[sqlx::test]
async fn dirty_and_empty_histories_are_not_repaired(pool: PgPool) -> Result<()> {
    sqlx::query("UPDATE _sqlx_migrations SET success=false")
        .execute(&pool)
        .await?;
    rejected(&pool).await?;
    sqlx::query("DELETE FROM _sqlx_migrations")
        .execute(&pool)
        .await?;
    rejected(&pool).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await?,
        0
    );
    Ok(())
}

#[sqlx::test]
async fn changed_structure_is_rejected_even_with_matching_history(pool: PgPool) -> Result<()> {
    sqlx::query("ALTER TABLE servers DROP COLUMN static_info_received_at")
        .execute(&pool)
        .await?;
    rejected(&pool).await?;
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM information_schema.columns WHERE table_name='servers' AND column_name='static_info_received_at'").fetch_one(&pool).await?, 0);
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn initialization_failure_rolls_back_every_table_and_can_be_retried(
    pool: PgPool,
) -> Result<()> {
    // Inject a DDL failure outside the application's public schema.
    sqlx::raw_sql("CREATE SCHEMA test_failure; CREATE FUNCTION test_failure.reject_index() RETURNS event_trigger LANGUAGE plpgsql AS $$ BEGIN IF TG_TAG='CREATE INDEX' THEN RAISE EXCEPTION 'TEST_ONLY failed DDL'; END IF; END $$; CREATE EVENT TRIGGER fail_initialization ON ddl_command_start EXECUTE FUNCTION test_failure.reject_index();").execute(&pool).await?;
    assert!(initialize(&pool).await.is_err());
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public'").fetch_one(&pool).await?, 0);
    sqlx::raw_sql("DROP EVENT TRIGGER fail_initialization; DROP SCHEMA test_failure CASCADE")
        .execute(&pool)
        .await?;
    initialize(&pool).await?;
    Ok(())
}

#[sqlx::test]
async fn current_defaults_cycles_and_listener_constraints_are_ready(pool: PgPool) -> Result<()> {
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT apsp_traffic_cycle_start(1728000000,31)")
            .fetch_one(&pool)
            .await?,
        1727654400
    );
    let source_agent: String = sqlx::query_scalar("SELECT column_default FROM information_schema.columns WHERE table_name='singbox_subscription_sources' AND column_name='user_agent'").fetch_one(&pool).await?;
    assert!(source_agent.contains("Apsp-subscription-import/1"));
    let id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO network_documents(id,kind,config,created_at,updated_at) VALUES($1,'forwarding',$2,1,1)").bind(id).bind(serde_json::json!({"owner":"apsp","enabled":true,"server_id":1,"listen_address":"127.0.0.1","listen_port":18080,"protocol":"tcp"})).execute(&pool).await?;
    let duplicate = sqlx::query("INSERT INTO network_documents(id,kind,config,created_at,updated_at) SELECT $1,kind,config,1,1 FROM network_documents WHERE id=$2").bind(uuid::Uuid::new_v4()).bind(id).execute(&pool).await.unwrap_err();
    assert!(duplicate.as_database_error().unwrap().is_unique_violation());
    Ok(())
}

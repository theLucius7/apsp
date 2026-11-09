use super::*;
use crate::config::Config;
use anyhow::Result;
use sqlx::PgPool;

#[sqlx::test(migrations = "../panel/migrations")]
async fn capability_and_its_preserved_old_flag_cannot_create_a_deployment(
    pool: PgPool,
) -> Result<()> {
    crate::users::install_without_users();
    let state = AppState::new(
        pool.clone(),
        Config {
            database_url: String::new(),
            listen: "127.0.0.1:0".parse()?,
            public_url: "http://127.0.0.1".into(),
            data_dir: std::env::temp_dir(),
            admin_password: Some("TEST_ONLY-plugin-publication-password".into()),
        },
    )
    .await?;
    let server: i64 = sqlx::query_scalar(
        "INSERT INTO servers(name,capabilities,dirty_at) VALUES('Candidate','[\"singbox\"]',0) RETURNING id",
    )
    .fetch_one(&pool)
    .await?;
    sqlx::query("INSERT INTO server_plugins(server_id,plugin,source,enabled_at) VALUES($1,'sing-box','agent_capability',0)")
        .bind(server).execute(&pool).await?;
    publish_server(&state, server, false).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM deployments WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?,
        0
    );
    // Capability changes must not turn the preserved old marker into a choice.
    sqlx::query("UPDATE servers SET capabilities='[]' WHERE id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    publish_server(&state, server, false).await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM deployments WHERE server_id=$1")
        .bind(server)
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 0);
    let pending: (i64, Option<i64>) =
        sqlx::query_as("SELECT manifest_rev,dirty_at FROM servers WHERE id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?;
    assert_eq!(pending, (0, Some(0)));

    // An explicit administrator choice remains eligible without device support.
    sqlx::query("UPDATE server_plugins SET source='administrator' WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    publish_server(&state, server, false).await?;
    let completed: (i64, Option<i64>) =
        sqlx::query_as("SELECT manifest_rev,dirty_at FROM servers WHERE id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?;
    assert_eq!(completed, (1, None));
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn stale_candidate_cannot_publish_after_enablement_is_withdrawn(
    pool: PgPool,
) -> anyhow::Result<()> {
    crate::users::install_without_users();
    let state = AppState::new(
        pool.clone(),
        Config {
            database_url: String::new(),
            listen: "127.0.0.1:0".parse()?,
            public_url: "http://127.0.0.1".into(),
            data_dir: std::env::temp_dir().join(format!("apsp-publish-{}", uuid::Uuid::new_v4())),
            admin_password: Some("test-only-plugin-publisher".into()),
        },
    )
    .await?;
    let server: i64 = sqlx::query_scalar("INSERT INTO servers(name,capabilities,dirty_at) VALUES('Candidate','[\"singbox\"]',0) RETURNING id").fetch_one(&pool).await?;
    sqlx::query("INSERT INTO server_plugins(server_id,plugin,source,enabled_at) VALUES($1,'sing-box','administrator',0)")
        .bind(server).execute(&pool).await?;
    let candidate: Option<i64> = sqlx::query_scalar(&format!("SELECT s.id FROM servers s LEFT JOIN server_plugins p ON p.server_id=s.id AND p.plugin='sing-box' WHERE s.id=$1 AND {DUE} AND ({}) IS NOT NULL",super::super::settings::SOURCE_SQL))
        .bind(server).fetch_optional(&pool).await?;
    assert_eq!(candidate, Some(server));
    // The explicit choice changes after candidate selection. Publication
    // must validate the locked server again instead of creating legacy proof.
    sqlx::query("UPDATE server_plugins SET enabled=FALSE WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    publish_server(&state, server, false).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM deployments WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?,
        0
    );
    let pending: (i64, Option<i64>) =
        sqlx::query_as("SELECT manifest_rev,dirty_at FROM servers WHERE id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?;
    assert_eq!(pending, (0, Some(0)));
    sqlx::query("UPDATE server_plugins SET enabled=TRUE WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    publish_server(&state, server, false).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM deployments WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>("SELECT dirty_at FROM servers WHERE id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?,
        None
    );
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn redeploy_publishes_the_unchanged_bundle_as_a_higher_revision(pool: PgPool) -> Result<()> {
    crate::users::install_without_users();
    let state = AppState::new(
        pool.clone(),
        Config {
            database_url: String::new(),
            listen: "127.0.0.1:0".parse()?,
            public_url: "http://127.0.0.1".into(),
            data_dir: std::env::temp_dir().join(format!("apsp-redeploy-{}", uuid::Uuid::new_v4())),
            admin_password: Some("TEST_ONLY-plugin-redeploy-password".into()),
        },
    )
    .await?;
    let server: i64 = sqlx::query_scalar(
        "INSERT INTO servers(name,capabilities,dirty_at) VALUES('Redeploy','[\"singbox\"]',0) RETURNING id",
    )
    .fetch_one(&pool)
    .await?;
    sqlx::query("INSERT INTO server_plugins(server_id,plugin,source,enabled_at) VALUES($1,'sing-box','administrator',0)")
        .bind(server).execute(&pool).await?;
    let revisions = || async {
        sqlx::query_as::<_, (i64, String)>(
            "SELECT rev,bundle_sha256 FROM deployments WHERE server_id=$1 ORDER BY rev",
        )
        .bind(server)
        .fetch_all(&pool)
        .await
    };
    publish_server(&state, server, false).await?;
    // An unchanged configuration reuses its revision on scheduled publication.
    sqlx::query("UPDATE servers SET dirty_at=0 WHERE id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    publish_server(&state, server, false).await?;
    assert_eq!(revisions().await?.len(), 1);

    // The administrator's redeploy needs no pending change and repeats the same bytes.
    assert_eq!(redeploy(&state, server).await?, Some(2));
    let published = revisions().await?;
    assert_eq!(published.len(), 2);
    assert_eq!(published[1].0, 2);
    assert_eq!(published[0].1, published[1].1);
    let current: (i64, Option<i64>, i64) = sqlx::query_as("SELECT s.manifest_rev,s.dirty_at,m.target_rev FROM servers s JOIN server_module_status m ON m.server_id=s.id AND m.module='singbox' WHERE s.id=$1")
        .bind(server).fetch_one(&pool).await?;
    assert_eq!(current, (2, None, 2));

    // Later scheduled publication keeps the redeployed revision.
    sqlx::query("UPDATE servers SET dirty_at=0 WHERE id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    publish_server(&state, server, false).await?;
    assert_eq!(revisions().await?.len(), 2);

    // A server that is no longer eligible gets nothing.
    sqlx::query("UPDATE server_plugins SET enabled=FALSE WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    assert_eq!(redeploy(&state, server).await?, None);
    assert_eq!(revisions().await?.len(), 2);
    Ok(())
}

use super::*;
use crate::config::Config;
use anyhow::Result;
use sqlx::PgPool;

/// A server that supports sing-box and has a change pending since the epoch.
async fn pending_server(pool: &PgPool, name: &str) -> Result<i64> {
    let server: i64 = sqlx::query_scalar(
        "INSERT INTO servers(name,capabilities) VALUES($1,'[\"singbox\"]') RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await?;
    sqlx::query("INSERT INTO runtime_publications(server_id,dirty_at) VALUES($1,0)")
        .bind(server)
        .execute(pool)
        .await?;
    Ok(server)
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn capability_and_its_preserved_old_flag_cannot_create_a_deployment(
    pool: PgPool,
) -> Result<()> {
    crate::users::install_without_users();
    let state = apsp_governance::state(
        pool.clone(),
        Config {
            database_url: String::new(),
            listen: "127.0.0.1:0".parse()?,
            public_url: "http://127.0.0.1".into(),
            data_dir: std::env::temp_dir(),
            admin_password: Some("TEST_ONLY-runtime-publication-password".into()),
        },
    )
    .await?;
    let server = pending_server(&pool, "Candidate").await?;
    sqlx::query("INSERT INTO runtime_enablements(server_id,module,source,enabled_at) VALUES($1,'singbox','agent_capability',0)")
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
        sqlx::query_as("SELECT manifest_rev,dirty_at FROM runtime_publications WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?;
    assert_eq!(pending, (0, Some(0)));

    // An explicit administrator choice remains eligible without device support.
    sqlx::query("UPDATE runtime_enablements SET source='administrator' WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    publish_server(&state, server, false).await?;
    let completed: (i64, Option<i64>) =
        sqlx::query_as("SELECT manifest_rev,dirty_at FROM runtime_publications WHERE server_id=$1")
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
    let state = apsp_governance::state(
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
    let server = pending_server(&pool, "Candidate").await?;
    sqlx::query("INSERT INTO runtime_enablements(server_id,module,source,enabled_at) VALUES($1,'singbox','administrator',0)")
        .bind(server).execute(&pool).await?;
    let candidate = crate::publications::due(&pool).await?;
    assert_eq!(candidate, vec![server]);
    // The explicit choice changes after candidate selection. Publication
    // must validate the locked server again instead of creating legacy proof.
    sqlx::query("UPDATE runtime_enablements SET enabled=FALSE WHERE server_id=$1")
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
        sqlx::query_as("SELECT manifest_rev,dirty_at FROM runtime_publications WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?;
    assert_eq!(pending, (0, Some(0)));
    sqlx::query("UPDATE runtime_enablements SET enabled=TRUE WHERE server_id=$1")
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
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT (SELECT dirty_at FROM runtime_publications WHERE server_id=$1)"
        )
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
    let state = apsp_governance::state(
        pool.clone(),
        Config {
            database_url: String::new(),
            listen: "127.0.0.1:0".parse()?,
            public_url: "http://127.0.0.1".into(),
            data_dir: std::env::temp_dir().join(format!("apsp-redeploy-{}", uuid::Uuid::new_v4())),
            admin_password: Some("TEST_ONLY-runtime-redeploy-password".into()),
        },
    )
    .await?;
    let server = pending_server(&pool, "Redeploy").await?;
    sqlx::query("INSERT INTO runtime_enablements(server_id,module,source,enabled_at) VALUES($1,'singbox','administrator',0)")
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
    sqlx::query("INSERT INTO runtime_publications(server_id,dirty_at) VALUES($1,0) ON CONFLICT(server_id) DO UPDATE SET dirty_at=0")
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
    let current: (i64, Option<i64>, i64) = sqlx::query_as("SELECT p.manifest_rev,p.dirty_at,m.target_rev FROM runtime_publications p JOIN server_module_status m ON m.server_id=p.server_id AND m.module='singbox' WHERE p.server_id=$1")
        .bind(server).fetch_one(&pool).await?;
    assert_eq!(current, (2, None, 2));

    // Later scheduled publication keeps the redeployed revision.
    sqlx::query("INSERT INTO runtime_publications(server_id,dirty_at) VALUES($1,0) ON CONFLICT(server_id) DO UPDATE SET dirty_at=0")
        .bind(server)
        .execute(&pool)
        .await?;
    publish_server(&state, server, false).await?;
    assert_eq!(revisions().await?.len(), 2);

    // A server that is no longer eligible gets nothing.
    sqlx::query("UPDATE runtime_enablements SET enabled=FALSE WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    assert_eq!(redeploy(&state, server).await?, None);
    assert_eq!(revisions().await?.len(), 2);
    Ok(())
}

/// The baseline adoption gives every server a publication row with the revision and the
/// pending mark copied from its server row. A new panel starting on such data publishes
/// nothing by itself: with nothing pending the cycle leaves deployments, revisions and
/// bundle digests as they are, and the next change continues from the copied revision
/// (Agents ignore a lower one, and deployments are keyed by revision).
#[sqlx::test(migrations = "../panel/migrations")]
async fn adopted_publication_state_publishes_nothing_until_a_change(pool: PgPool) -> Result<()> {
    crate::users::install_without_users();
    let state = apsp_governance::state(
        pool.clone(),
        Config {
            database_url: String::new(),
            listen: "127.0.0.1:0".parse()?,
            public_url: "http://127.0.0.1".into(),
            data_dir: std::env::temp_dir().join(format!("apsp-adopted-{}", uuid::Uuid::new_v4())),
            admin_password: Some("TEST_ONLY-adopted-publication-password".into()),
        },
    )
    .await?;
    let adopted = pending_server(&pool, "Adopted").await?;
    sqlx::query("INSERT INTO runtime_enablements(server_id,module,source,enabled_at) VALUES($1,'singbox','administrator',0)")
        .bind(adopted).execute(&pool).await?;
    sqlx::query("INSERT INTO deployments(server_id,module,rev,bundle,bundle_sha256,created_at) VALUES($1,'singbox',41,'{\"files\":{}}',$2,1)")
        .bind(adopted).bind("a".repeat(64)).execute(&pool).await?;
    sqlx::query("INSERT INTO server_module_status(server_id,module,target_rev,applied_rev,last_result_rev,healthy,updated_at) VALUES($1,'singbox',41,41,41,TRUE,1)")
        .bind(adopted).execute(&pool).await?;
    sqlx::query("UPDATE runtime_publications SET manifest_rev=41,dirty_at=NULL WHERE server_id=$1")
        .bind(adopted)
        .execute(&pool)
        .await?;
    // A server enabled after the adoption that was never marked has no publication row.
    let fresh: i64 = sqlx::query_scalar(
        "INSERT INTO servers(name,capabilities) VALUES('Fresh','[\"singbox\"]') RETURNING id",
    )
    .fetch_one(&pool)
    .await?;
    sqlx::query("INSERT INTO runtime_enablements(server_id,module,source,enabled_at) VALUES($1,'singbox','administrator',0)")
        .bind(fresh).execute(&pool).await?;
    let published = || async {
        sqlx::query_scalar::<_, serde_json::Value>("SELECT jsonb_build_object('deployments',(SELECT jsonb_agg(jsonb_build_array(server_id,module,rev,bundle_sha256) ORDER BY server_id,rev) FROM deployments),'status',(SELECT jsonb_agg(to_jsonb(m) ORDER BY server_id) FROM server_module_status m),'publications',(SELECT jsonb_agg(to_jsonb(p) ORDER BY server_id) FROM runtime_publications p))")
            .fetch_one(&pool)
            .await
    };
    let before = published().await?;
    for _ in 0..2 {
        publish_due(&state, None).await?;
    }
    assert_eq!(published().await?, before);
    let mut connection = pool.acquire().await?;
    assert_eq!(
        crate::publications::revision(&mut connection, adopted).await?,
        41
    );
    assert_eq!(
        crate::publications::revision(&mut connection, fresh).await?,
        0
    );
    assert!(!crate::publications::pending(&mut connection, adopted).await?);
    assert!(!crate::publications::pending(&mut connection, fresh).await?);
    drop(connection);

    // A change is published as the revision after the copied one.
    let mut tx = pool.begin().await?;
    crate::business::lock_publication(&mut tx).await?;
    crate::business::mark_dirty(&mut tx, &[adopted]).await?;
    tx.commit().await?;
    sqlx::query("UPDATE runtime_publications SET dirty_at=0 WHERE server_id=$1")
        .bind(adopted)
        .execute(&pool)
        .await?;
    publish_due(&state, None).await?;
    let revisions: Vec<i64> =
        sqlx::query_scalar("SELECT rev FROM deployments WHERE server_id=$1 ORDER BY rev")
            .bind(adopted)
            .fetch_all(&pool)
            .await?;
    assert_eq!(revisions, [41, 42]);
    let current: (i64, Option<i64>, i64) = sqlx::query_as("SELECT p.manifest_rev,p.dirty_at,m.target_rev FROM runtime_publications p JOIN server_module_status m ON m.server_id=p.server_id AND m.module='singbox' WHERE p.server_id=$1")
        .bind(adopted).fetch_one(&pool).await?;
    assert_eq!(current, (42, None, 42));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM deployments WHERE server_id=$1")
            .bind(fresh)
            .fetch_one(&pool)
            .await?,
        0
    );
    Ok(())
}

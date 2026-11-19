use super::*;
use crate::config::Config;
use anyhow::Result;
use sqlx::PgPool;

async fn pending_server(pool: &PgPool, name: &str) -> Result<i64> {
    let server: i64 = sqlx::query_scalar(
        "INSERT INTO servers(name,capabilities) VALUES($1,'[\"agent:self-contained:v1\",\"singbox\"]') RETURNING id",
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

async fn insert_node(pool: &PgPool, server: i64, port: i32, enabled: bool) -> Result<i64> {
    let (private_key, public_key) = crate::business::generate_reality_keypair();
    let short_id = crate::business::short_id();
    Ok(sqlx::query_scalar(
        "INSERT INTO nodes(name,server_id,protocol,port,public_host,sni,private_key,public_key,short_id,protocol_config,enabled,settings) \
         VALUES('TEST_ONLY node',$1,'vless-reality',$2,'proxy.example.invalid','www.example.invalid',$3,$4,$5,'{\"type\":\"vless-reality\"}',$6,'{}') RETURNING id",
    )
    .bind(server)
    .bind(port)
    .bind(private_key)
    .bind(public_key)
    .bind(short_id)
    .bind(enabled)
    .fetch_one(pool)
    .await?)
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn capability_or_server_profile_alone_never_creates_a_deployment(pool: PgPool) -> Result<()> {
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
    let server = pending_server(&pool, "Monitor").await?;
    sqlx::query("INSERT INTO singbox_server_profiles(server_id,definition,revision,updated_at) VALUES($1,'{}',1,1)")
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
    let settled: (i64, Option<i64>) =
        sqlx::query_as("SELECT manifest_rev,dirty_at FROM runtime_publications WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?;
    assert_eq!(settled, (0, None));

    // Saving an enabled node makes the server configured even while offline or without an Agent.
    insert_node(&pool, server, 20443, true).await?;
    sqlx::query("UPDATE runtime_publications SET dirty_at=0 WHERE server_id=$1")
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
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM deployments WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?,
        1
    );
    Ok(())
}

#[sqlx::test(migrations = "../panel/migrations")]
async fn disabling_last_node_publishes_empty_manifest_and_reenabling_mints_new_revision(
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
    let node = insert_node(&pool, server, 20443, true).await?;
    let candidate = crate::publications::due(&pool).await?;
    assert_eq!(candidate, vec![server]);
    publish_server(&state, server, false).await?;
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT p.manifest_rev,m.target_rev FROM runtime_publications p JOIN server_module_status m ON m.server_id=p.server_id AND m.module='singbox' WHERE p.server_id=$1",
        )
        .bind(server)
        .fetch_one(&pool)
        .await?,
        (1, 1)
    );

    // Disabling the last node deactivates target_rev to 0 and advances manifest_rev without
    // inserting a fake deployment bundle.
    sqlx::query("UPDATE nodes SET enabled=FALSE WHERE id=$1")
        .bind(node)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE runtime_publications SET dirty_at=0 WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    publish_server(&state, server, false).await?;
    assert_eq!(
        sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT p.manifest_rev,m.target_rev,(SELECT COUNT(*) FROM deployments WHERE server_id=$1) FROM runtime_publications p JOIN server_module_status m ON m.server_id=p.server_id AND m.module='singbox' WHERE p.server_id=$1",
        )
        .bind(server)
        .fetch_one(&pool)
        .await?,
        (2, 0, 1)
    );

    // Re-enabling the exact same node mints revision 3 instead of reusing revision 1.
    sqlx::query("UPDATE nodes SET enabled=TRUE WHERE id=$1")
        .bind(node)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE runtime_publications SET dirty_at=0 WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    publish_server(&state, server, false).await?;
    assert_eq!(
        sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT p.manifest_rev,m.target_rev,(SELECT COUNT(*) FROM deployments WHERE server_id=$1) FROM runtime_publications p JOIN server_module_status m ON m.server_id=p.server_id AND m.module='singbox' WHERE p.server_id=$1",
        )
        .bind(server)
        .fetch_one(&pool)
        .await?,
        (3, 3, 2)
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
    let node = insert_node(&pool, server, 20443, true).await?;
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

    // A server whose service is disabled gets nothing on redeploy.
    sqlx::query("UPDATE nodes SET enabled=FALSE WHERE id=$1")
        .bind(node)
        .execute(&pool)
        .await?;
    assert_eq!(redeploy(&state, server).await?, None);
    assert_eq!(revisions().await?.len(), 2);
    Ok(())
}

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
    insert_node(&pool, adopted, 20443, true).await?;
    sqlx::query("INSERT INTO deployments(server_id,module,rev,bundle,bundle_sha256,created_at) VALUES($1,'singbox',41,'{\"files\":{}}',$2,1)")
        .bind(adopted).bind("a".repeat(64)).execute(&pool).await?;
    sqlx::query("INSERT INTO server_module_status(server_id,module,target_rev,applied_rev,last_result_rev,healthy,updated_at) VALUES($1,'singbox',41,41,41,TRUE,1)")
        .bind(adopted).execute(&pool).await?;
    sqlx::query("UPDATE runtime_publications SET manifest_rev=41,dirty_at=NULL WHERE server_id=$1")
        .bind(adopted)
        .execute(&pool)
        .await?;
    let fresh: i64 = sqlx::query_scalar(
        "INSERT INTO servers(name,capabilities) VALUES('Fresh','[\"agent:self-contained:v1\",\"singbox\"]') RETURNING id",
    )
    .fetch_one(&pool)
    .await?;
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

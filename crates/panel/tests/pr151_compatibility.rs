#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use business_support::TestPanel;
use reqwest::Method;
use serde_json::Value;
use sqlx::{PgPool, migrate::Migrator};
use std::borrow::Cow;
use uuid::Uuid;

async fn numeric_history(pool: &PgPool) -> Result<Value> {
    // Compare every original source column, without treating additive schema
    // defaults as mutations of the old persisted numeric identity or secrets.
    // Versions and hops retain their complete immutable row projections.
    Ok(sqlx::query_scalar(
        "SELECT jsonb_build_object(
            'sources',COALESCE((SELECT jsonb_agg(to_jsonb(s) ORDER BY id) FROM (
                SELECT id,name,kind,secret_url,secret_authorization,secret_content,
                    source_host,settings_revision,identity_epoch,refresh_interval_seconds,
                    archived,deleted_at,current_revision_id,last_attempt_at,last_success_at,
                    next_refresh_at,last_error,etag,last_modified,cache_settings_revision,
                    cache_identity_epoch,created_at
                FROM singbox_subscription_sources
            ) s),'[]'::jsonb),
            'versions',COALESCE((SELECT jsonb_agg(to_jsonb(v) ORDER BY chain_id,generation)
                FROM singbox_chain_versions v),'[]'::jsonb),
            'hops',COALESCE((SELECT jsonb_agg(to_jsonb(h) ORDER BY chain_id,generation,position)
                FROM singbox_chain_hops h),'[]'::jsonb))",
    )
    .fetch_one(pool)
    .await?)
}

async fn source_workbench_defaults(pool: &PgPool) -> Result<Value> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(jsonb_agg(jsonb_build_object(
            'id',id,'auto_refresh',auto_refresh,'user_agent',user_agent,
            'traffic',traffic,'changes',changes) ORDER BY id),'[]'::jsonb)
         FROM singbox_subscription_sources",
    )
    .fetch_one(pool)
    .await?)
}

#[sqlx::test(migrations = false)]
async fn existing_main_38_upgrades_without_redefining_numeric_or_immutable_history(
    pool: PgPool,
) -> Result<()> {
    let all = sqlx::migrate!();
    let old = Migrator {
        migrations: Cow::Owned(all.iter().filter(|m| m.version <= 38).cloned().collect()),
        ..Migrator::DEFAULT
    };
    old.run(&pool).await?;
    let server: i64 =
        sqlx::query_scalar("INSERT INTO servers(name) VALUES('TEST_ONLY old main38') RETURNING id")
            .fetch_one(&pool)
            .await?;
    let entry: i64 = sqlx::query_scalar("INSERT INTO nodes(name,server_id,port,public_host,sni,private_key,public_key,short_id) VALUES('TEST_ONLY entry',$1,443,'entry.example.test','www.example.test','private','public','0123abcd') RETURNING id").bind(server).fetch_one(&pool).await?;
    let exit: i64 = sqlx::query_scalar("INSERT INTO nodes(name,server_id,port,public_host,sni,private_key,public_key,short_id) VALUES('TEST_ONLY exit',$1,8443,'exit.example.test','www.example.test','private','public','0123abcd') RETURNING id").bind(server).fetch_one(&pool).await?;
    let relay = Uuid::new_v4();
    let chain: i64 = sqlx::query_scalar("INSERT INTO singbox_chains(name,entry_node_id,exit_node_id,relay_uuid) VALUES('TEST_ONLY preserved',$1,$2,$3) RETURNING id").bind(entry).bind(exit).bind(relay).fetch_one(&pool).await?;
    sqlx::query("INSERT INTO singbox_chain_versions(chain_id,generation,legacy,path_json,semantic_hash,networks,stage,created_at,updated_at) VALUES($1,1,TRUE,'{\"TEST_ONLY\":\"old immutable numeric path\"}','preserved','{\"tcp\":true,\"udp\":true}','active',1,1)").bind(chain).execute(&pool).await?;
    sqlx::query("INSERT INTO singbox_chain_hops(chain_id,generation,position,kind,managed_node_id,managed_server_id,endpoint_json,relay_uuid) VALUES($1,1,0,'managed',$2,$3,'{}',$4)").bind(chain).bind(exit).bind(server).bind(relay).execute(&pool).await?;
    sqlx::query("UPDATE singbox_chains SET active_generation=1 WHERE id=$1")
        .bind(chain)
        .execute(&pool)
        .await?;
    let source: i64 = sqlx::query_scalar("INSERT INTO singbox_subscription_sources(name,kind,secret_content,created_at) VALUES('TEST_ONLY existing numeric source','inline','preserved private fixture',1) RETURNING id").fetch_one(&pool).await?;
    let before = numeric_history(&pool).await?;
    let original_sources: Value = sqlx::query_scalar(
        "SELECT COALESCE(jsonb_agg(to_jsonb(s) ORDER BY id),'[]'::jsonb)
         FROM singbox_subscription_sources s",
    )
    .fetch_one(&pool)
    .await?;
    // This proves the explicit projection omits no column from main38.
    assert_eq!(before["sources"], original_sources);
    let expected_defaults = serde_json::json!([{
        "id": source,
        "auto_refresh": true,
        "user_agent": "Apsp-subscription-import/1",
        "traffic": {},
        "changes": {"added": 0, "missing": 0, "unsupported": 0, "updated": 0}
    }]);
    all.run(&pool).await?;
    assert_eq!(numeric_history(&pool).await?, before);
    assert_eq!(source_workbench_defaults(&pool).await?, expected_defaults);
    let types: Vec<(String,String)> = sqlx::query_as("SELECT table_name,data_type FROM information_schema.columns WHERE table_schema='public' AND column_name='id' AND table_name IN ('singbox_external_nodes','singbox_ordered_external_nodes') ORDER BY table_name").fetch_all(&pool).await?;
    assert_eq!(
        types,
        vec![
            ("singbox_external_nodes".into(), "bigint".into()),
            ("singbox_ordered_external_nodes".into(), "uuid".into())
        ]
    );
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM singbox_ordered_chain_versions WHERE chain_id=$1 AND generation=1 AND legacy").bind(chain).fetch_one(&pool).await?, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM singbox_live_chains WHERE id=$1 AND path_kind='legacy'"
        )
        .bind(chain)
        .fetch_one(&pool)
        .await?,
        1
    );
    all.run(&pool).await?;
    assert_eq!(numeric_history(&pool).await?, before);
    assert_eq!(source_workbench_defaults(&pool).await?, expected_defaults);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await?,
        all.iter().count() as i64
    );
    Ok(())
}

#[sqlx::test]
async fn retiring_ordered_entry_is_never_reclassified_as_a_flat_direct_resource(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel
        .create_server(&cookie, "TEST_ONLY namespace guard")
        .await?;
    let node = panel
        .create_node(&cookie, server, "TEST_ONLY reserved ordered entry")
        .await?;
    let id = node["id"].as_i64().unwrap();
    // Model an unresolved, soft-deleted ordered lineage. The old API must not
    // treat pending device cleanup as proof that the dedicated entry is free.
    sqlx::query("INSERT INTO singbox_chains(name,entry_node_id,path_kind,phase,deleted_at) VALUES('TEST_ONLY retained',$1,'ordered','retiring',1)").bind(id).execute(&panel.state.pool).await?;
    let flat: Value = panel
        .admin(
            Method::GET,
            "/api/plugins/sing-box/proxy-resources",
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        flat.as_array()
            .unwrap()
            .iter()
            .all(|row| !(row["kind"] == "direct" && row["id"] == id))
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM singbox_live_chains WHERE entry_node_id=$1"
        )
        .bind(id)
        .fetch_one(&panel.state.pool)
        .await?,
        0
    );
    Ok(())
}

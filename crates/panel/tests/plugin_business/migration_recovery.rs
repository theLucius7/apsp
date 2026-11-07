use anyhow::{Context, Result};
use serde_json::{Value, json};
use sqlx::{
    PgPool,
    migrate::{Migration, Migrator},
};
use std::{borrow::Cow, collections::BTreeMap};

pub(super) async fn legacy_snapshot(pool: &PgPool) -> Result<BTreeMap<&'static str, Value>> {
    let mut snapshot = BTreeMap::new();
    for table in [
        "servers",
        "nodes",
        "users",
        "accesses",
        "deployments",
        "server_module_status",
        "usage_batches",
        "usage_records",
        "sessions",
        "enrollment_tokens",
    ] {
        // Administrator login creates its own session; device credentials are
        // the imported state that must survive the business ownership move.
        let filter = if table == "sessions" {
            " WHERE server_id IS NOT NULL"
        } else {
            ""
        };
        let value = sqlx::query_scalar(&format!(
            "SELECT COALESCE(jsonb_agg(to_jsonb(r) ORDER BY to_jsonb(r)::text),'[]'::jsonb) FROM {table} r{filter}"
        )).fetch_one(pool).await?;
        snapshot.insert(table, value);
    }
    Ok(snapshot)
}

pub(super) fn append_expected_server_defaults(
    snapshot: &mut BTreeMap<&'static str, Value>,
) -> Result<()> {
    let servers = snapshot
        .get_mut("servers")
        .and_then(Value::as_array_mut)
        .context("legacy servers")?;
    // Extend only the expected legacy state; compare every actual column unchanged.
    for server in servers {
        server["asset_settings"] = json!({});
        server["telemetry_settings"] = json!({"persist_interval_secs":60});
        server["static_info_received_at"] = Value::Null;
        server["probe_revision"] = json!(0);
        server["probe_fingerprint"] = Value::Null;
        server["last_contact_at"] = Value::Null;
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn failed_enablement_backfill_rolls_back_and_can_be_retried_idempotently(
    pool: PgPool,
) -> Result<()> {
    let all = sqlx::migrate!();
    let old = Migrator {
        migrations: Cow::Owned(all.iter().filter(|m| m.version < 12).cloned().collect()),
        ..Migrator::DEFAULT
    };
    old.run(&pool).await?;
    let server: i64 = sqlx::query_scalar("INSERT INTO servers(name,device_public_key,capabilities,manifest_rev) VALUES('Old enabled server','TEST_ONLY-device-key','[\"singbox\"]',7) RETURNING id")
        .fetch_one(&pool).await?;
    sqlx::query("INSERT INTO deployments(server_id,module,rev,bundle,bundle_sha256,created_at) VALUES($1,'singbox',7,'preserved-bundle','preserved-hash',1234)")
        .bind(server).execute(&pool).await?;
    let mut before = legacy_snapshot(&pool).await?;
    let migration = all
        .iter()
        .find(|m| m.version == 12)
        .context("enablement migration")?;
    assert!(
        !migration.no_tx,
        "enablement backfill must be transactional"
    );
    // Fail after the actual DDL and backfill, using SQLx's real migration path.
    let failed = Migration::new(
        migration.version,
        migration.description.clone(),
        migration.migration_type,
        Cow::Owned(format!("{}\nSELECT 1/0;", migration.sql)),
        migration.no_tx,
    );
    let mut interrupted = old.migrations.to_vec();
    interrupted.push(failed);
    let interrupted = Migrator {
        migrations: Cow::Owned(interrupted),
        ..Migrator::DEFAULT
    };
    let mut failed_connection = pool.acquire().await?;
    assert!(
        interrupted
            .run_direct(&mut *failed_connection)
            .await
            .is_err()
    );
    // Failed startup drops its database session, releasing SQLx's advisory lock.
    failed_connection.close().await?;
    let table: Option<String> = sqlx::query_scalar("SELECT to_regclass('server_plugins')::text")
        .fetch_one(&pool)
        .await?;
    assert_eq!(table, None);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations WHERE version=12")
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(legacy_snapshot(&pool).await?, before);

    all.run(&pool).await?;
    // Later server columns add only their explicit legacy defaults.
    append_expected_server_defaults(&mut before)?;
    let enabled: (i64, String, bool) = sqlx::query_as(
        "SELECT server_id,source,enabled FROM server_plugins WHERE plugin='sing-box'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(enabled, (server, "agent_capability".into(), true));
    let stamp: i64 = sqlx::query_scalar("SELECT enabled_at FROM server_plugins WHERE server_id=$1")
        .bind(server)
        .fetch_one(&pool)
        .await?;
    all.run(&pool).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM server_plugins")
            .fetch_one(&pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT enabled_at FROM server_plugins WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?,
        stamp
    );
    assert_eq!(legacy_snapshot(&pool).await?, before);
    Ok(())
}

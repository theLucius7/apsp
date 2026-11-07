//! Initializes an empty development database or verifies the current baseline.

use anyhow::{Context, Result, ensure};
use sqlx::PgPool;

const REBUILD: &str = "数据库不是当前开发基线或结构不完整，已拒绝启动。请先备份，再手动重建开发数据库；程序不会自动删除数据，也不支持历史增量升级。";
const FINGERPRINT: &str = include_str!("../../panel/schema_fingerprint.sql");

pub async fn initialize(pool: &PgPool) -> Result<()> {
    let mut tx = pool.begin().await?;
    // Transaction-scoped locking also releases on cancellation or initialization failure.
    sqlx::query("SELECT pg_advisory_xact_lock(73401220)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL search_path TO public")
        .execute(&mut *tx)
        .await?;
    let tracked: bool =
        sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
            .fetch_one(&mut *tx)
            .await?;
    let mut baseline = sqlx::migrate!("../panel/migrations");
    ensure!(
        baseline.iter().count() == 1,
        "开发阶段只允许一份当前数据库基线"
    );
    baseline.set_locking(false);
    if tracked {
        let applied: Vec<(i64, bool, Vec<u8>)> = sqlx::query_as(
            "SELECT version,success,checksum FROM public._sqlx_migrations ORDER BY version",
        )
        .fetch_all(&mut *tx)
        .await
        .context(REBUILD)?;
        ensure!(applied.len() == 1, REBUILD);
        let expected = baseline
            .iter()
            .next()
            .context("missing database baseline")?;
        ensure!(
            applied[0].0 == expected.version
                && applied[0].1
                && applied[0].2 == expected.checksum.as_ref(),
            REBUILD
        );
    } else {
        let objects: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public') + (SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public')")
            .fetch_one(&mut *tx).await?;
        ensure!(objects == 0, REBUILD);
        baseline
            .run_direct(&mut *tx)
            .await
            .context("初始化当前数据库基线失败；事务已回滚")?;
    }
    let saved: String =
        sqlx::query_scalar("SELECT fingerprint FROM public.apsp_schema_baseline WHERE singleton")
            .fetch_one(&mut *tx)
            .await
            .context(REBUILD)?;
    let actual: String = sqlx::query_scalar(FINGERPRINT)
        .fetch_one(&mut *tx)
        .await
        .context(REBUILD)?;
    ensure!(saved == actual, REBUILD);
    tx.commit().await?;
    Ok(())
}

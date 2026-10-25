//! Preparation that must run before the migration sequence.
//!
//! Released migrations are never edited because their checksums are recorded.
//! When a released migration cannot apply to data a later feature froze, the
//! assembly crate, which owns the migration sequence, prepares that data first.

use sqlx::PgPool;

/// Runs before `AppState::new` applies pending migrations.
pub async fn prepare(pool: &PgPool) -> anyhow::Result<()> {
    let tracked: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(pool)
        .await?;
    if !tracked {
        return Ok(());
    }
    rename_archived_user_agents(pool).await
}

/// Migration 0065 renames the default User-Agent of numbered subscription
/// sources. Once the source migration has run, those rows are a read-only
/// archive whose trigger rejects the update and aborts the whole upgrade. The
/// same rename is applied first with the guard briefly disabled, so 0065 finds
/// nothing left to change.
async fn rename_archived_user_agents(pool: &PgPool) -> anyhow::Result<()> {
    let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version=62 AND success) AND NOT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version=65)")
        .fetch_one(pool)
        .await?;
    if !pending {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    let archived: bool = sqlx::query_scalar("SELECT singbox_sources_migrated()")
        .fetch_one(&mut *tx)
        .await?;
    if archived {
        sqlx::query("ALTER TABLE singbox_subscription_sources DISABLE TRIGGER singbox_numbered_source_archive")
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE singbox_subscription_sources SET user_agent='Apsp-subscription-import/1' WHERE user_agent='Sinan-subscription-import/1'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("ALTER TABLE singbox_subscription_sources ENABLE TRIGGER singbox_numbered_source_archive")
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

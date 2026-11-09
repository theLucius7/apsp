//! The resource side of an account refresh pass: claiming the pass, reading the current
//! rows and recording what the cloud reported for a resource.

use super::{
    account_on, lock,
    model::{Account, Resource},
};
use crate::error::ApiResult;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

/// Claims an account's refresh pass, or `None` when the account is disabled or not due.
/// Polls at most eight resources per pass, rotating by last attempt, including failures.
pub(super) async fn claim_refresh(
    pool: &PgPool,
    id: Uuid,
    now: i64,
) -> ApiResult<Option<(Account, Vec<Resource>)>> {
    let mut tx = lock(pool, id).await?;
    let account = account_on(&mut tx, id).await?;
    if !account.enabled || account.next_run_at > now {
        return Ok(None);
    }
    sqlx::query("UPDATE alicloud_accounts SET last_attempt_at=$2,next_run_at=$3 WHERE id=$1")
        .bind(id)
        .bind(now)
        .bind(now + 300)
        .execute(&mut *tx)
        .await?;
    let resources: Vec<Resource> = sqlx::query_as("SELECT * FROM alicloud_resources WHERE account_id=$1 AND NOT archived ORDER BY last_attempt_at,id LIMIT 8")
        .bind(id).fetch_all(&mut *tx).await?;
    let ids: Vec<Uuid> = resources.iter().map(|resource| resource.id).collect();
    sqlx::query("UPDATE alicloud_resources SET last_attempt_at=$2 WHERE id=ANY($1)")
        .bind(&ids)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some((account, resources)))
}

/// The account as stored now, or `None` once it is archived.
pub(super) async fn current_account(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> ApiResult<Option<Account>> {
    Ok(
        sqlx::query_as("SELECT * FROM alicloud_accounts WHERE id=$1 AND NOT archived")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?,
    )
}

/// The resource as stored now, or `None` once it is archived.
pub(super) async fn current_resource(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> ApiResult<Option<Resource>> {
    Ok(
        sqlx::query_as("SELECT * FROM alicloud_resources WHERE id=$1 AND NOT archived")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?,
    )
}

/// Records that the cloud could not describe a resource.
pub(super) async fn snapshot_failed(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    code: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE alicloud_resources SET error_code=$2 WHERE id=$1")
        .bind(id)
        .bind(code)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Accounts whose refresh is due, the earliest first; one per tick.
pub(super) async fn due_accounts(pool: &PgPool, now: i64) -> ApiResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar("SELECT id FROM alicloud_accounts WHERE enabled AND NOT archived AND next_run_at<=$1 ORDER BY next_run_at,id LIMIT 1")
        .bind(now).fetch_all(pool).await?)
}

/// The registered accounts and resources, by name.
pub(super) async fn listed(pool: &PgPool) -> ApiResult<(Vec<Account>, Vec<Resource>)> {
    let accounts =
        sqlx::query_as("SELECT * FROM alicloud_accounts WHERE NOT archived ORDER BY name,id")
            .fetch_all(pool)
            .await?;
    let resources =
        sqlx::query_as("SELECT * FROM alicloud_resources WHERE NOT archived ORDER BY name,id")
            .fetch_all(pool)
            .await?;
    Ok((accounts, resources))
}

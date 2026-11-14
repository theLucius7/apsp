//! The publication state the traffic service keeps for each server (`runtime_publications`):
//! the revision of the configuration last published to it and, while a change awaits
//! publication, when that change was marked. A server without a row has published nothing
//! (revision 0) and has nothing pending.
//!
//! This is the one place that answers "is a publication pending" and "which revision is
//! current". Traffic's own queries, the later owners (access, operations, the application)
//! and resources' server views (through `server_runtime::ServerRuntime`) all ask it.
//! Revisions only grow: deployments are keyed by server, module and revision, and an Agent
//! ignores a manifest whose revision is lower than one it has seen.

use crate::{
    servers::{self, RowLock},
    settings::{ENABLEMENT_JOIN, SOURCE_SQL},
};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use std::collections::HashMap;

/// The current time in the epoch milliseconds a pending change is marked with.
const NOW_MS: &str = "FLOOR(EXTRACT(EPOCH FROM clock_timestamp())*1000)::bigint";
/// A pending change (of the publication row `p`) is published once it was left alone for
/// five seconds.
const DUE: &str = "p.dirty_at <= FLOOR(EXTRACT(EPOCH FROM clock_timestamp())*1000)::bigint - 5000";

/// SQL that holds while a change to the server `server` (a parameter or a qualified column,
/// such as `"$1"` or `"s.id"`) awaits publication.
pub fn pending_sql(server: &str) -> String {
    format!(
        "EXISTS(SELECT 1 FROM runtime_publications publication WHERE publication.server_id={server} AND publication.dirty_at IS NOT NULL)"
    )
}

/// SQL for when the change to `server` that awaits publication was marked, in epoch
/// milliseconds; NULL while nothing is pending.
pub fn pending_since_sql(server: &str) -> String {
    format!(
        "(SELECT publication.dirty_at FROM runtime_publications publication WHERE publication.server_id={server})"
    )
}

/// SQL for the revision last published to `server`, 0 before the first.
pub fn revision_sql(server: &str) -> String {
    format!(
        "COALESCE((SELECT publication.manifest_rev FROM runtime_publications publication WHERE publication.server_id={server}),0)"
    )
}

/// Whether a change to `server` awaits publication, as seen in the caller's connection or
/// transaction.
pub async fn pending(connection: &mut PgConnection, server: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar(&format!("SELECT {}", pending_sql("$1")))
        .bind(server)
        .fetch_one(connection)
        .await
}

/// The revision last published to `server`, 0 before the first.
pub async fn revision(connection: &mut PgConnection, server: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar(&format!("SELECT {}", revision_sql("$1")))
        .bind(server)
        .fetch_one(connection)
        .await
}

/// The revision last published to each of the servers `ids` that has published one; the
/// others are at 0.
pub async fn revisions(pool: &PgPool, ids: &[i64]) -> sqlx::Result<HashMap<i64, i64>> {
    Ok(sqlx::query_as::<_, (i64, i64)>(
        "SELECT server_id,manifest_rev FROM runtime_publications WHERE server_id=ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect())
}

/// Marks a change to each of the servers `ids` that is not deleted as pending publication
/// now; the publisher waits until none was marked for five seconds. The server rows are held
/// FOR KEY SHARE through resources, in id order, so deletion and retirement wait while
/// contact times are still written; the upsert then locks the publication rows in the same
/// order.
pub(crate) async fn mark_pending_tx(
    tx: &mut Transaction<'_, Postgres>,
    ids: &[i64],
) -> sqlx::Result<()> {
    let live = servers::lock_all(tx, ids, RowLock::KeyShare).await?;
    if !live.is_empty() {
        sqlx::query(&format!(
            "INSERT INTO runtime_publications(server_id,dirty_at) SELECT server_id,{NOW_MS} FROM unnest($1::bigint[]) AS marked(server_id) ORDER BY server_id ON CONFLICT(server_id) DO UPDATE SET dirty_at=EXCLUDED.dirty_at"
        ))
        .bind(&live)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Marks the first publication of `server` as pending when nothing was ever deployed to it,
/// keeping an earlier mark. The caller holds the server row.
pub(crate) async fn mark_first_pending_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> sqlx::Result<()> {
    sqlx::query(&format!(
        "INSERT INTO runtime_publications(server_id,dirty_at) SELECT $1,{NOW_MS} WHERE NOT EXISTS(SELECT 1 FROM deployments WHERE server_id=$1 AND module='singbox') ON CONFLICT(server_id) DO UPDATE SET dirty_at=COALESCE(runtime_publications.dirty_at,EXCLUDED.dirty_at)"
    ))
    .bind(server)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Servers whose pending change is due, that are not deleted and have an enabled runtime,
/// in id order.
pub(crate) async fn due(pool: &PgPool) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(&format!(
        "SELECT s.id FROM runtime_publications p JOIN servers s ON s.id=p.server_id {ENABLEMENT_JOIN} WHERE s.deleted_at IS NULL AND {DUE} AND ({SOURCE_SQL}) IS NOT NULL ORDER BY s.id"
    ))
    .fetch_all(pool)
    .await
}

/// Claims the publication of `server` in the caller's publication transaction and returns
/// the revision last published. The server row is held FOR KEY SHARE (its deletion and
/// retirement wait, its contact times are still written) and the publication row
/// FOR UPDATE. `None` when the server is deleted, has no enabled runtime, has nothing due
/// (unless `force`) or another transaction holds its publication row; it is left to a later
/// pass.
pub(crate) async fn claim_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    force: bool,
) -> sqlx::Result<Option<i64>> {
    if servers::lock_row(tx, server, RowLock::KeyShare, "id")
        .await?
        .is_none()
    {
        return Ok(None);
    }
    if force {
        // An administrator's redeploy may come before anything was ever pending.
        sqlx::query(
            "INSERT INTO runtime_publications(server_id) VALUES($1) ON CONFLICT(server_id) DO NOTHING",
        )
        .bind(server)
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query_scalar(&format!(
        "SELECT p.manifest_rev FROM runtime_publications p JOIN servers s ON s.id=p.server_id {ENABLEMENT_JOIN} WHERE p.server_id=$1 AND ({DUE} OR $2) AND ({SOURCE_SQL}) IS NOT NULL FOR UPDATE OF p SKIP LOCKED"
    ))
    .bind(server)
    .bind(force)
    .fetch_optional(&mut **tx)
    .await
}

/// Records that the configuration of a claimed server is current without a new revision.
pub(crate) async fn settle_tx(tx: &mut Transaction<'_, Postgres>, server: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE runtime_publications SET dirty_at=NULL WHERE server_id=$1")
        .bind(server)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Records that `revision` was published to a claimed server; nothing is pending any more.
pub(crate) async fn record_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    revision: i64,
) -> sqlx::Result<()> {
    let updated = sqlx::query(
        "UPDATE runtime_publications SET manifest_rev=$2,dirty_at=NULL WHERE server_id=$1",
    )
    .bind(server)
    .bind(revision)
    .execute(&mut **tx)
    .await?;
    match updated.rows_affected() {
        1 => Ok(()),
        _ => Err(sqlx::Error::RowNotFound),
    }
}

/// Locks the publication state of `server` for a revision published outside the publication
/// cycle (the empty first installation) and returns the revision last published. The
/// caller holds the server row.
pub(crate) async fn lock_tx(tx: &mut Transaction<'_, Postgres>, server: i64) -> sqlx::Result<i64> {
    sqlx::query(
        "INSERT INTO runtime_publications(server_id) VALUES($1) ON CONFLICT(server_id) DO NOTHING",
    )
    .bind(server)
    .execute(&mut **tx)
    .await?;
    sqlx::query_scalar(
        "SELECT manifest_rev FROM runtime_publications WHERE server_id=$1 FOR UPDATE",
    )
    .bind(server)
    .fetch_one(&mut **tx)
    .await
}

/// Records `revision` as the last one published to a server locked with [`lock_tx`]; a
/// pending change stays pending.
pub(crate) async fn advance_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    revision: i64,
) -> sqlx::Result<()> {
    let updated = sqlx::query("UPDATE runtime_publications SET manifest_rev=$2 WHERE server_id=$1")
        .bind(server)
        .bind(revision)
        .execute(&mut **tx)
        .await?;
    match updated.rows_affected() {
        1 => Ok(()),
        _ => Err(sqlx::Error::RowNotFound),
    }
}

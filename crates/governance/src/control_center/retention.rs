//! The record retention policy (系统自检 › 分类保留期限): how many days each kind of
//! record is kept. Governance keeps the policy and purges its own records here; every
//! other owner purges its own records in its own background task, with the cutoffs and
//! the cadence ([`Purges`]) of this module, and reads or mirrors its period through
//! [`days`] and [`set`]. Each owner purges in one transaction.

use sqlx::{PgExecutor, PgPool};
use std::future::Future;

/// Retention periods are whole days, so a purge once an hour is soon enough.
const PURGE_EVERY: i64 = 3600;

/// A kind of record the policy keeps for a number of days.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The request audit, independent observer reports and expired API tokens.
    Audit,
    /// IP quality results.
    Ip,
    /// Closed terminal sessions with their input and output.
    Terminal,
    /// Server events and the contents of configuration history, fixed device operations
    /// and command output.
    Logs,
    /// The records of proxy access, such as subscription fetches.
    ProxyAccess,
    /// Monitoring history; edited in the panel settings, which mirror it here.
    Monitoring,
}

impl Kind {
    const ALL: [Self; 6] = [
        Self::Audit,
        Self::Ip,
        Self::Terminal,
        Self::Logs,
        Self::ProxyAccess,
        Self::Monitoring,
    ];

    /// The kind as the policy stores it.
    fn key(self) -> &'static str {
        match self {
            Self::Audit => "audit",
            Self::Ip => "ip",
            Self::Terminal => "terminal",
            Self::Logs => "logs",
            Self::ProxyAccess => "proxy-access",
            Self::Monitoring => "monitoring",
        }
    }

    /// The kind a stored key names.
    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }
}

/// The days the policy keeps `kind`, if the policy has a row for it.
pub async fn days<'e>(
    executor: impl PgExecutor<'e>,
    kind: Kind,
) -> Result<Option<i32>, sqlx::Error> {
    sqlx::query_scalar("SELECT days FROM record_retention_policy WHERE kind=$1")
        .bind(kind.key())
        .fetch_optional(executor)
        .await
}

/// Sets the days the policy keeps `kind`; false when the policy has no row for it.
pub async fn set<'e>(
    executor: impl PgExecutor<'e>,
    kind: Kind,
    days: i32,
    now: i64,
) -> Result<bool, sqlx::Error> {
    Ok(
        sqlx::query("UPDATE record_retention_policy SET days=$2,updated_at=$3 WHERE kind=$1")
            .bind(kind.key())
            .bind(days)
            .bind(now)
            .execute(executor)
            .await?
            .rows_affected()
            == 1,
    )
}

/// The time before which records of `kind` are past their retention period at `now`.
pub async fn cutoff(pool: &PgPool, kind: Kind, now: i64) -> Result<i64, sqlx::Error> {
    let days: i32 = sqlx::query_scalar("SELECT days FROM record_retention_policy WHERE kind=$1")
        .bind(kind.key())
        .fetch_one(pool)
        .await?;
    Ok(now - i64::from(days) * 86400)
}

/// When one owner's background task purges: on its first pass, then an hour after each
/// purge that succeeded; a failed purge is due again on the task's next pass. The task
/// keeps it in its context.
#[derive(Debug, Default)]
pub struct Purges {
    next: i64,
}

impl Purges {
    /// Runs `purge` when one is due at `now`. A purge that is not due is dropped without
    /// being polled, so it does no work.
    pub async fn run<E>(
        &mut self,
        now: i64,
        purge: impl Future<Output = Result<(), E>>,
    ) -> Result<(), E> {
        if now >= self.next {
            purge.await?;
            self.next = now + PURGE_EVERY;
        }
        Ok(())
    }
}

/// Purges governance's own records past the audit period: the request audit, independent
/// observer reports, and API tokens that expired (and, if revoked, were revoked) before
/// the cutoff.
pub async fn purge(pool: &PgPool, now: i64) -> Result<(), sqlx::Error> {
    let audit = cutoff(pool, Kind::Audit, now).await?;
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM management_audit WHERE occurred_at<$1")
        .bind(audit)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM system_observer_reports WHERE received_at<$1")
        .bind(audit)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM management_api_tokens WHERE expires_at<$1 AND (revoked_at IS NULL OR revoked_at<$1)").bind(audit).execute(&mut *tx).await?;
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::Purges;

    #[tokio::test]
    async fn purges_run_hourly_and_retry_a_failed_one_on_the_next_pass() {
        let mut purges = Purges::default();
        let mut runs = Vec::new();
        let mut failures = Vec::new();
        for (now, result) in [
            (100, Ok(())),
            (3699, Ok(())),
            (3700, Err("unavailable")),
            (3760, Ok(())),
            (7359, Ok(())),
            (7360, Ok(())),
        ] {
            let outcome = purges
                .run(now, async {
                    runs.push(now);
                    result
                })
                .await;
            if outcome.is_err() {
                failures.push(now);
            }
        }
        assert_eq!(runs, [100, 3700, 3760, 7360], "the purges that ran");
        assert_eq!(failures, [3700], "a failed purge is reported");
    }
}

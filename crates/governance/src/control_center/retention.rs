//! The record retention policy (系统自检 › 分类保留期限): how many days each kind of
//! record is kept. Governance keeps the policy and purges its own records here; every
//! other owner purges its own records in its own background task, with the cutoffs and
//! the cadence ([`Purges`]) of this module. Each owner purges in one transaction.

use sqlx::PgPool;
use std::future::Future;

/// Retention periods are whole days, so a purge once an hour is soon enough.
const PURGE_EVERY: i64 = 3600;

/// A kind of record the policy keeps for a number of days and its owner purges in the
/// background.
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
}

impl Kind {
    /// The kind as the policy stores it.
    fn key(self) -> &'static str {
        match self {
            Self::Audit => "audit",
            Self::Ip => "ip",
            Self::Terminal => "terminal",
            Self::Logs => "logs",
        }
    }
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

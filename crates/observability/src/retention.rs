//! Observability's records past the retention policy governance keeps: IP quality results
//! (kind `ip`) and the server event log (kind `logs`). The history maintenance task
//! purges them hourly (`telemetry::run`).

use crate::control_center::retention::{Kind, cutoff};
use sqlx::PgPool;

/// Purges observability's records past their period at `now`, in one transaction. A
/// stored IP quality result goes once it is past the period and none of its provider
/// datasets is left.
pub async fn purge(pool: &PgPool, now: i64) -> Result<(), sqlx::Error> {
    let ip = cutoff(pool, Kind::Ip, now).await?;
    let logs = cutoff(pool, Kind::Logs, now).await?;
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM server_ip_quality_datasets WHERE checked_at<$1")
        .bind(ip)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM server_ip_quality WHERE checked_at<$1 AND NOT EXISTS(SELECT 1 FROM server_ip_quality_datasets d WHERE d.server_id=server_ip_quality.server_id AND d.ip=server_ip_quality.ip AND d.provider=server_ip_quality.provider)").bind(ip).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM fleet_events WHERE occurred_at<$1")
        .bind(logs)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

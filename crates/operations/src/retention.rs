//! Operations' records past the retention policy governance keeps: closed terminal
//! sessions with their input and output (kind `terminal`), and the contents of
//! configuration history, finished or reconciled fixed device operations and finished
//! command output (kind `logs`); those rows stay with their contents cleared.

use crate::control_center::retention::{Kind, Purges, cutoff};
use apsp_panel_host::maintenance::{Cadence, Pass, outcome, periodic};
use apsp_protocol::now_timestamp;
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;

/// Purges on the retention cadence under the heartbeat `operations-retention`, which is
/// recorded once a minute.
pub async fn run(pool: PgPool) {
    periodic(
        "operations-retention",
        pool.clone(),
        Cadence::every(Duration::from_secs(60)),
        (pool, Purges::default()),
        |(pool, purges)| Box::pin(pass(pool, purges)),
    )
    .await
}

async fn pass(pool: &PgPool, purges: &mut Purges) -> Pass {
    let now = now_timestamp();
    outcome(
        purges.run(now, purge(pool, now)).await,
        json!({"source":"retention-cycle"}),
    )
}

/// Purges operations' records past their period at `now`, in one transaction.
pub async fn purge(pool: &PgPool, now: i64) -> Result<(), sqlx::Error> {
    let terminal = cutoff(pool, Kind::Terminal, now).await?;
    let logs = cutoff(pool, Kind::Logs, now).await?;
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM fleet_terminal_inputs i USING fleet_terminal_sessions s WHERE i.session_id=s.id AND s.status IN ('closed','failed') AND s.expires_at<$1").bind(terminal).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM fleet_terminal_outputs o USING fleet_terminal_sessions s WHERE o.session_id=s.id AND s.status IN ('closed','failed') AND s.expires_at<$1").bind(terminal).execute(&mut *tx).await?;
    sqlx::query(
        "DELETE FROM fleet_terminal_sessions WHERE status IN ('closed','failed') AND expires_at<$1",
    )
    .bind(terminal)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE fleet_config_history SET content='' WHERE created_at<$1 AND content<>''")
        .bind(logs)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE fleet_operations SET operation=CASE WHEN operation ? 'content' THEN jsonb_set(operation,'{content}',to_jsonb(''::text)) ELSE operation END,result=NULL WHERE requested_at<$1 AND (status IN ('succeeded','failed','expired','cancelled') OR reconciled_at IS NOT NULL) AND (COALESCE(operation->>'content','')<>'' OR result IS NOT NULL)").bind(logs).execute(&mut *tx).await?;
    sqlx::query("UPDATE remote_commands SET result=jsonb_set(jsonb_set(result,'{stdout}',to_jsonb('[按保留策略清理]'::text)),'{stderr}',to_jsonb('[按保留策略清理]'::text)) WHERE finished_at<$1 AND state IN ('succeeded','failed','cancelled','expired','interrupted') AND result IS NOT NULL AND (COALESCE(result->>'stdout','') NOT IN ('','[按保留策略清理]') OR COALESCE(result->>'stderr','') NOT IN ('','[按保留策略清理]'))").bind(logs).execute(&mut *tx).await?;
    tx.commit().await
}

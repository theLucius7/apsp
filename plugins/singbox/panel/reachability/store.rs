//! Current state, history and run scheduling in PostgreSQL.

use super::{
    alerts,
    classify::{ReachabilityState, Tracked, Transition, advance},
    targets::Target,
};
use crate::error::ApiResult;
use apsp_panel_host::settings as panel_settings;
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use std::{collections::HashMap, net::IpAddr};
use uuid::Uuid;

pub const MINIMUM_INTERVAL_MINUTES: i32 = 15;
pub const MAXIMUM_INTERVAL_MINUTES: i32 = 1440;
/// Covers any run the budget can admit; the worker also clears it when it restarts.
const LEASE_SECS: i64 = 3600;
const HISTORY_RETENTION_SECS: i64 = 30 * 86400;
const EVENT_RETENTION_SECS: i64 = 90 * 86400;
const STALE_TARGET_SECS: i64 = 30 * 86400;
const CLEANUP_BATCH: i64 = 5000;

#[derive(Clone, Debug, FromRow)]
pub struct Settings {
    pub enabled: bool,
    pub interval_minutes: i32,
    pub credential_id: Option<Uuid>,
    pub revision: i64,
    pub next_run_at: i64,
    pub run_requested_at: Option<i64>,
    pub lease_until: Option<i64>,
    pub last_run: Value,
}

const SETTINGS_COLUMNS: &str = "enabled,interval_minutes,credential_id,revision,next_run_at,run_requested_at,lease_until,last_run";

pub async fn read_settings(pool: &PgPool) -> ApiResult<Settings> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SETTINGS_COLUMNS} FROM singbox_reachability_settings WHERE singleton"
    ))
    .fetch_one(pool)
    .await?)
}

pub async fn lock_settings(connection: &mut sqlx::PgConnection) -> ApiResult<Settings> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SETTINGS_COLUMNS} FROM singbox_reachability_settings WHERE singleton FOR UPDATE"
    ))
    .fetch_one(connection)
    .await?)
}

pub async fn is_enabled(pool: &PgPool) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT enabled FROM singbox_reachability_settings WHERE singleton")
            .fetch_one(pool)
            .await?,
    )
}

pub struct Claim {
    pub settings: Settings,
    pub manual: bool,
}

/// Takes the run if one is due. A requested run overrides the schedule, not the
/// opt-in: nothing is ever claimed while the monitor is disabled.
pub async fn claim_run(pool: &PgPool, now: i64) -> ApiResult<Option<Claim>> {
    let mut tx = pool.begin().await?;
    let settings = lock_settings(&mut tx).await?;
    let manual = settings.run_requested_at.is_some();
    if !settings.enabled
        || settings.lease_until.is_some_and(|until| until > now)
        || (!manual && settings.next_run_at > now)
    {
        return Ok(None);
    }
    sqlx::query(
        "UPDATE singbox_reachability_settings SET lease_until=$1,run_requested_at=NULL WHERE singleton",
    )
    .bind(now + LEASE_SECS)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(Claim { settings, manual }))
}

pub async fn finish_run(pool: &PgPool, summary: &Value, next_run_at: i64) -> ApiResult<()> {
    sqlx::query(
        "UPDATE singbox_reachability_settings SET lease_until=NULL,next_run_at=$1,last_run=$2 WHERE singleton",
    )
    .bind(next_run_at)
    .bind(summary)
    .execute(pool)
    .await?;
    Ok(())
}

/// Only one worker exists, so a lease it finds at startup belongs to a dead attempt.
pub async fn release_lease(pool: &PgPool) -> ApiResult<()> {
    sqlx::query("UPDATE singbox_reachability_settings SET lease_until=NULL WHERE singleton")
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Clone, Debug, FromRow)]
pub struct TargetRow {
    pub id: i64,
    pub host: String,
    pub port: i32,
    pub state: String,
    pub pending_state: Option<String>,
    pub failing_streak: i32,
    pub observed_state: Option<String>,
    pub resolved_ip: Option<String>,
    pub detail: Value,
    pub checked_at: Option<i64>,
    pub state_changed_at: Option<i64>,
    pub deferred_at: Option<i64>,
}

const TARGET_COLUMNS: &str = "id,host,port,state,pending_state,failing_streak,observed_state,resolved_ip,detail,checked_at,state_changed_at,deferred_at";

impl TargetRow {
    pub fn current_state(&self) -> ReachabilityState {
        ReachabilityState::parse(&self.state).unwrap_or(ReachabilityState::Unknown)
    }
    pub fn pending(&self) -> Option<ReachabilityState> {
        self.pending_state
            .as_deref()
            .and_then(ReachabilityState::parse)
    }
    fn tracked(&self) -> Tracked {
        Tracked {
            state: self.current_state(),
            pending: self.pending(),
            failing_streak: u32::try_from(self.failing_streak).unwrap_or(0),
        }
    }
    /// Likely to need the control comparison again on its next check.
    pub fn failing(&self) -> bool {
        self.current_state().is_blocked() || self.failing_streak > 0
    }
}

pub type RowMap = HashMap<(String, u16), TargetRow>;

pub async fn target_rows(pool: &PgPool) -> ApiResult<RowMap> {
    let rows: Vec<TargetRow> = sqlx::query_as(&format!(
        "SELECT {TARGET_COLUMNS} FROM singbox_reachability_targets"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let port = u16::try_from(row.port).ok()?;
            Some(((row.host.clone(), port), row))
        })
        .collect())
}

pub async fn target_row(pool: &PgPool, host: &str, port: u16) -> ApiResult<Option<TargetRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {TARGET_COLUMNS} FROM singbox_reachability_targets WHERE host=$1 AND port=$2"
    ))
    .bind(host)
    .bind(i32::from(port))
    .fetch_optional(pool)
    .await?)
}

/// Ensures every current endpoint has a row and notes that it is still in use.
pub async fn sync_targets(pool: &PgPool, targets: &[Target], now: i64) -> ApiResult<RowMap> {
    let hosts: Vec<&str> = targets.iter().map(|target| target.host.as_str()).collect();
    let ports: Vec<i32> = targets
        .iter()
        .map(|target| i32::from(target.port))
        .collect();
    sqlx::query(
        "INSERT INTO singbox_reachability_targets(host,port,seen_at) SELECT h,p,$3::bigint FROM unnest($1::text[],$2::int[]) AS t(h,p) ON CONFLICT(host,port) DO UPDATE SET seen_at=EXCLUDED.seen_at",
    )
    .bind(&hosts)
    .bind(&ports)
    .bind(now)
    .execute(pool)
    .await?;
    target_rows(pool).await
}

pub struct Observation<'a> {
    pub target: &'a Target,
    pub row_id: i64,
    pub observed: ReachabilityState,
    pub ip: Option<IpAddr>,
    pub detail: &'a Value,
    pub now: i64,
}

/// Stores one run's result, applying the debounce under the row lock and
/// recording any alert in the same transaction.
pub async fn record(pool: &PgPool, observation: Observation<'_>) -> ApiResult<Transition> {
    let panel = panel_settings::read(pool).await?;
    let mut tx = pool.begin().await?;
    let row: TargetRow = sqlx::query_as(&format!(
        "SELECT {TARGET_COLUMNS} FROM singbox_reachability_targets WHERE id=$1 FOR UPDATE"
    ))
    .bind(observation.row_id)
    .fetch_one(&mut *tx)
    .await?;
    let (next, transition) = advance(row.tracked(), observation.observed);
    sqlx::query(
        "UPDATE singbox_reachability_targets SET state=$2,pending_state=$3,failing_streak=$4,observed_state=$5,resolved_ip=$6,detail=$7,checked_at=$8,deferred_at=NULL,state_changed_at=CASE WHEN state<>$2 OR state_changed_at IS NULL THEN $8 ELSE state_changed_at END WHERE id=$1",
    )
    .bind(observation.row_id)
    .bind(next.state.as_str())
    .bind(next.pending.map(ReachabilityState::as_str))
    .bind(i32::try_from(next.failing_streak).unwrap_or(i32::MAX))
    .bind(observation.observed.as_str())
    .bind(observation.ip.map(|ip| ip.to_string()))
    .bind(observation.detail)
    .bind(observation.now)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO singbox_reachability_history(target_id,checked_at,state,observed_state,detail) VALUES($1,$2,$3,$4,$5)",
    )
    .bind(observation.row_id)
    .bind(observation.now)
    .bind(next.state.as_str())
    .bind(observation.observed.as_str())
    .bind(observation.detail)
    .execute(&mut *tx)
    .await?;
    alerts::record(
        &mut tx,
        &panel,
        observation.target,
        observation.row_id,
        transition,
        observation.detail,
        observation.now,
    )
    .await?;
    tx.commit().await?;
    Ok(transition)
}

pub async fn mark_deferred(pool: &PgPool, ids: &[i64], now: i64) -> ApiResult<()> {
    sqlx::query("UPDATE singbox_reachability_targets SET deferred_at=$2 WHERE id=ANY($1)")
        .bind(ids)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}

/// Bounded retention. Rows for removed endpoints stay a while so a node recreated
/// on the same address keeps its debounce state, then disappear with their history.
pub async fn cleanup(pool: &PgPool, now: i64) -> ApiResult<()> {
    sqlx::query("DELETE FROM singbox_reachability_history WHERE id IN (SELECT id FROM singbox_reachability_history WHERE checked_at<$1 ORDER BY checked_at LIMIT $2)")
        .bind(now - HISTORY_RETENTION_SECS)
        .bind(CLEANUP_BATCH)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM singbox_reachability_targets WHERE seen_at<$1")
        .bind(now - STALE_TARGET_SECS)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM singbox_reachability_events WHERE id IN (SELECT e.id FROM singbox_reachability_events e WHERE e.created_at<$1 AND NOT EXISTS(SELECT 1 FROM singbox_reachability_deliveries d WHERE d.event_id=e.id AND d.state IN ('pending','sending')) ORDER BY e.created_at LIMIT $2)")
        .bind(now - EVENT_RETENTION_SECS)
        .bind(CLEANUP_BATCH)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn recent_events(pool: &PgPool) -> ApiResult<Vec<Value>> {
    Ok(sqlx::query_scalar(
        "SELECT jsonb_build_object('id',e.id,'kind',e.kind,'from_state',e.from_state,'to_state',e.to_state,'message',e.message,'created_at',e.created_at,'deliveries',COALESCE((SELECT jsonb_agg(jsonb_build_object('channel',d.channel,'state',d.state,'attempts',d.attempts,'last_error',d.last_error) ORDER BY d.id) FROM singbox_reachability_deliveries d WHERE d.event_id=e.id),'[]'::jsonb)) FROM singbox_reachability_events e ORDER BY e.created_at DESC,e.id LIMIT 50",
    )
    .fetch_all(pool)
    .await?)
}

//! The monitoring history policy and what observability keeps for each server in
//! `telemetry_servers`: the telemetry settings its Agent follows and its newest metrics. A
//! server without a row has the default settings and no stored metrics.

use super::*;
use crate::control_center::retention;
use apsp_protocol::telemetry::TelemetrySettings;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgConnection, Postgres, Transaction};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryPolicy {
    pub history_retention_days: i32,
}

pub(super) async fn read_policy(pool: &sqlx::PgPool) -> ApiResult<HistoryPolicy> {
    Ok(HistoryPolicy {
        history_retention_days: sqlx::query_scalar(
            "SELECT history_retention_days FROM telemetry_policy WHERE singleton",
        )
        .fetch_one(pool)
        .await?,
    })
}

pub async fn policy(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<HistoryPolicy>> {
    auth::require_admin(&state, &headers).await?;
    Ok(Json(read_policy(&state.pool).await?))
}

pub async fn update_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(policy): Json<HistoryPolicy>,
) -> ApiResult<Json<HistoryPolicy>> {
    auth::require_admin(&state, &headers).await?;
    if !(1..=3650).contains(&policy.history_retention_days) {
        return Err(ApiError::BadRequest("历史保存期限须为 1–3650 天".into()));
    }
    // Shortening the history removes samples at the next cleanup, so it needs
    // the same fresh proof as the other record retention changes.
    if policy.history_retention_days < read_policy(&state.pool).await?.history_retention_days {
        crate::control_center::require_recent_proof(&state, &headers).await?;
    }
    // This page is the only editor of the monitoring history; the record
    // retention table mirrors it so the self-check reports the same value.
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE telemetry_policy SET history_retention_days=$1 WHERE singleton")
        .bind(policy.history_retention_days)
        .execute(&mut *tx)
        .await?;
    retention::set(
        &mut *tx,
        retention::Kind::Monitoring,
        policy.history_retention_days,
        apsp_protocol::now_timestamp(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(policy))
}

/// The newest metrics stored for a server.
#[derive(Clone, Debug, PartialEq)]
pub struct LatestMetrics {
    /// Milliseconds since the epoch; 0 when the device did not say when it sampled them.
    pub sampled_at: i64,
    /// Every metric of the newest sample, process and pressure detail included.
    pub metrics: Value,
}

impl Default for LatestMetrics {
    /// What a server without stored metrics shows: no sample time and no metrics.
    fn default() -> Self {
        Self {
            sampled_at: 0,
            metrics: json!({}),
        }
    }
}

/// The newest stored metrics of each of `servers` that has any.
pub async fn latest_metrics(
    connection: &mut PgConnection,
    servers: &[i64],
) -> ApiResult<HashMap<i64, LatestMetrics>> {
    let rows: Vec<(i64, i64, Value)> = sqlx::query_as(
        "SELECT server_id,sampled_at,latest_metrics FROM telemetry_servers WHERE server_id=ANY($1)",
    )
    .bind(servers)
    .fetch_all(connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(server, sampled_at, metrics)| {
            (
                server,
                LatestMetrics {
                    sampled_at,
                    metrics,
                },
            )
        })
        .collect())
}

/// When the newest stored metrics of each of `servers` were sampled, for those whose
/// stored metrics say so.
pub async fn sampled_at(
    connection: &mut PgConnection,
    servers: &[i64],
) -> ApiResult<HashMap<i64, i64>> {
    Ok(sqlx::query_as(
        "SELECT server_id,sampled_at FROM telemetry_servers WHERE server_id=ANY($1) AND sampled_at<>0",
    )
    .bind(servers)
    .fetch_all(connection)
    .await?
    .into_iter()
    .collect())
}

/// The telemetry settings of each of `servers` as saved, the defaults where none are.
pub async fn settings_of(pool: &sqlx::PgPool, servers: &[i64]) -> ApiResult<HashMap<i64, Value>> {
    let mut saved: HashMap<i64, Value> =
        sqlx::query_as("SELECT server_id,settings FROM telemetry_servers WHERE server_id=ANY($1)")
            .bind(servers)
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
    Ok(servers
        .iter()
        .map(|server| {
            let settings = saved
                .remove(server)
                .unwrap_or_else(|| json!(TelemetrySettings::default()));
            (*server, settings)
        })
        .collect())
}

/// Saves the telemetry settings of a server that exists, in the caller's transaction.
pub async fn save_settings_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    settings: &TelemetrySettings,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO telemetry_servers(server_id,settings) VALUES($1,$2) ON CONFLICT(server_id) DO UPDATE SET settings=EXCLUDED.settings")
        .bind(server)
        .bind(serde_json::to_value(settings).map_err(anyhow::Error::from)?)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Gives `target` the telemetry settings of `source`, as saved, in the caller's transaction.
pub async fn copy_settings_tx(
    tx: &mut Transaction<'_, Postgres>,
    source: i64,
    target: i64,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO telemetry_servers(server_id,settings) VALUES($2,COALESCE((SELECT settings FROM telemetry_servers WHERE server_id=$1),$3)) ON CONFLICT(server_id) DO UPDATE SET settings=EXCLUDED.settings")
        .bind(source)
        .bind(target)
        .bind(json!(TelemetrySettings::default()))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(super) async fn read_storage_settings(
    state: &AppState,
    server: i64,
) -> ApiResult<Json<TelemetrySettings>> {
    let saved: Option<Value> = sqlx::query_scalar(
        "SELECT t.settings FROM servers s LEFT JOIN telemetry_servers t ON t.server_id=s.id WHERE s.id=$1 AND s.deleted_at IS NULL",
    )
    .bind(server)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::NotFound)?;
    Ok(Json(match saved {
        Some(value) => serde_json::from_value(value).map_err(anyhow::Error::from)?,
        None => TelemetrySettings::default(),
    }))
}

pub async fn storage_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(server): Path<i64>,
) -> ApiResult<Json<TelemetrySettings>> {
    auth::require_admin(&state, &headers).await?;
    read_storage_settings(&state, server).await
}

pub async fn agent_storage_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<TelemetrySettings>> {
    let server = crate::device_channel::require_agent(&state, &headers).await?;
    read_storage_settings(&state, server).await
}

pub async fn update_storage_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(server): Path<i64>,
    Json(settings): Json<TelemetrySettings>,
) -> ApiResult<Json<TelemetrySettings>> {
    auth::require_admin(&state, &headers).await?;
    if !settings.valid() {
        return Err(ApiError::BadRequest("历史写入间隔须为 15–3600 秒".into()));
    }
    let changed = sqlx::query("INSERT INTO telemetry_servers(server_id,settings) SELECT id,$2 FROM servers WHERE id=$1 AND deleted_at IS NULL ON CONFLICT(server_id) DO UPDATE SET settings=EXCLUDED.settings")
        .bind(server)
        .bind(serde_json::to_value(&settings).map_err(anyhow::Error::from)?)
        .execute(&state.pool)
        .await?;
    if changed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(Json(settings))
}

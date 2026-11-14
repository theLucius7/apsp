//! What resources needs from observability (ADR 0128 section 5.5): the network traffic
//! summary and live telemetry shown with servers, the newest stored metrics and the
//! telemetry settings of servers (observability keeps both in its own tables), checks for
//! the probes and alert rules that a new server or an enrollment template brings, and the
//! latency task assignment of new or migrated servers. Resources declares it,
//! observability answers and the application connects it. Later owners that show a
//! server's newest metrics with the server read them here too.

use apsp_protocol::{ProbeSpec, telemetry::TelemetrySettings};
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use std::collections::HashMap;

use crate::{
    AppState,
    error::{ApiError, ApiResult},
    ports::{BoxFuture, Port},
    servers::Server,
};

/// The newest metrics observability stored for a server.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredMetrics {
    /// Milliseconds since the epoch; 0 when the device did not say when it sampled them.
    pub sampled_at: i64,
    /// Every metric of the newest sample, process and pressure detail included.
    pub metrics: Value,
}

impl Default for StoredMetrics {
    /// What a server without stored metrics shows: no sample time and no metrics.
    fn default() -> Self {
        Self {
            sampled_at: 0,
            metrics: json!({}),
        }
    }
}

pub trait ServerObservation: Send + Sync {
    /// Fills each server's traffic summary for its current billing cycle.
    fn attach_traffic<'a>(
        &'a self,
        pool: &'a PgPool,
        servers: &'a mut [Server],
        now: i64,
    ) -> BoxFuture<'a, anyhow::Result<()>>;

    /// Fills each server's stored metrics, replaces them with newer live samples and adds
    /// telemetry settings.
    fn attach_live<'a>(
        &'a self,
        state: &'a AppState,
        servers: &'a mut [Server],
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// The newest stored metrics of each of `servers` that has any, in the caller's
    /// connection or transaction.
    fn stored_metrics<'a>(
        &'a self,
        connection: &'a mut PgConnection,
        servers: &'a [i64],
    ) -> BoxFuture<'a, ApiResult<HashMap<i64, StoredMetrics>>>;

    /// The telemetry settings of each of `servers` as saved; servers without saved
    /// settings have the defaults.
    fn telemetry_settings<'a>(
        &'a self,
        pool: &'a PgPool,
        servers: &'a [i64],
    ) -> BoxFuture<'a, ApiResult<HashMap<i64, Value>>>;

    /// Saves the telemetry settings a new server starts with, in the transaction that
    /// creates it.
    fn save_telemetry_settings_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        settings: &'a TelemetrySettings,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// Gives `target` the telemetry settings of `source` when a collection migrates.
    fn copy_telemetry_settings_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        source: i64,
        target: i64,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// Normalizes a probe before it is saved.
    fn prepare_probe(&self, spec: &mut ProbeSpec) -> ApiResult<()>;

    /// Checks an alert rule that an enrollment template applies to its servers.
    fn check_alert_rule(&self, rule: &Value) -> ApiResult<()>;

    /// Serializes server changes with the assignment of latency tasks.
    fn lock_latency_tasks_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// Assigns the default latency tasks to a new server.
    fn assign_default_latency_tasks_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
    ) -> BoxFuture<'a, ApiResult<()>>;
}

static OBSERVATION: Port<dyn ServerObservation> = Port::new();

pub fn install(observation: &'static dyn ServerObservation) {
    OBSERVATION.install(observation);
}

fn required() -> ApiResult<&'static dyn ServerObservation> {
    OBSERVATION.get().ok_or_else(|| {
        ApiError::Conflict("服务器观测的提供方未登记，不能显示或设置服务器的监测".into())
    })
}

pub async fn attach_traffic(pool: &PgPool, servers: &mut [Server], now: i64) -> ApiResult<()> {
    Ok(required()?.attach_traffic(pool, servers, now).await?)
}

pub async fn attach_live(state: &AppState, servers: &mut [Server]) -> ApiResult<()> {
    required()?.attach_live(state, servers).await
}

/// The newest stored metrics of `servers`; a server without any is left out (see
/// [`StoredMetrics::default`]).
pub async fn stored_metrics(
    connection: &mut PgConnection,
    servers: &[i64],
) -> ApiResult<HashMap<i64, StoredMetrics>> {
    required()?.stored_metrics(connection, servers).await
}

pub async fn telemetry_settings(pool: &PgPool, servers: &[i64]) -> ApiResult<HashMap<i64, Value>> {
    required()?.telemetry_settings(pool, servers).await
}

pub async fn save_telemetry_settings_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    settings: &TelemetrySettings,
) -> ApiResult<()> {
    required()?
        .save_telemetry_settings_tx(tx, server, settings)
        .await
}

pub async fn copy_telemetry_settings_tx(
    tx: &mut Transaction<'_, Postgres>,
    source: i64,
    target: i64,
) -> ApiResult<()> {
    required()?
        .copy_telemetry_settings_tx(tx, source, target)
        .await
}

pub fn prepare_probe(spec: &mut ProbeSpec) -> ApiResult<()> {
    required()?.prepare_probe(spec)
}

pub fn check_alert_rule(rule: &Value) -> ApiResult<()> {
    required()?.check_alert_rule(rule)
}

pub async fn lock_latency_tasks_tx(tx: &mut Transaction<'_, Postgres>) -> ApiResult<()> {
    required()?.lock_latency_tasks_tx(tx).await
}

pub async fn assign_default_latency_tasks_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<()> {
    required()?
        .assign_default_latency_tasks_tx(tx, server)
        .await
}

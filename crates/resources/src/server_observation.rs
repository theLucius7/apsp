//! What resources needs from observability (ADR 0128 section 5.5): the network traffic
//! summary and live telemetry shown with servers, checks for the probes and alert rules
//! that a new server or an enrollment template brings, and the latency task assignment
//! of new or migrated servers. Resources declares it, observability answers and the
//! application connects it.

use apsp_protocol::ProbeSpec;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};

use crate::{
    AppState,
    error::{ApiError, ApiResult},
    ports::{BoxFuture, Port},
    servers::Server,
};

pub trait ServerObservation: Send + Sync {
    /// Fills each server's traffic summary for its current billing cycle.
    fn attach_traffic<'a>(
        &'a self,
        pool: &'a PgPool,
        servers: &'a mut [Server],
        now: i64,
    ) -> BoxFuture<'a, anyhow::Result<()>>;

    /// Replaces stored metrics with newer live samples and adds telemetry settings.
    fn attach_live<'a>(
        &'a self,
        state: &'a AppState,
        servers: &'a mut [Server],
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

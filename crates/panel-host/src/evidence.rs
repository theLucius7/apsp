//! Facts that domains report to observability (ADR 0128 section 3.2): events on a
//! server's timeline and incidents that need an administrator.
//!
//! Resources, governance and costs come before observability in the dependency order,
//! so they report through here; observability answers and the application connects it.
//! Without a connection a report fails with the transaction it belongs to, so a fact is
//! never dropped silently.

use serde_json::Value;
use sqlx::{Postgres, Transaction};

use crate::{
    error::{ApiError, ApiResult},
    ports::{BoxFuture, Port},
};

/// A condition that needs attention. Reports with the same key update one open incident.
pub struct IncidentReport<'a> {
    pub key: &'a str,
    pub server: Option<i64>,
    pub title: &'a str,
    pub severity: &'a str,
    pub evidence: Value,
    pub observed_at: i64,
}

pub trait Evidence: Send + Sync {
    fn record_event_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        kind: &'a str,
        detail: Value,
    ) -> BoxFuture<'a, ApiResult<()>>;

    fn open_incident_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        report: IncidentReport<'a>,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// Resolves the open incidents of a key with the evidence that the condition ended.
    fn recover_incident_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        key: &'a str,
        evidence: Value,
        observed_at: i64,
        notify: bool,
    ) -> BoxFuture<'a, ApiResult<()>>;
}

static EVIDENCE: Port<dyn Evidence> = Port::new();

pub fn install(evidence: &'static dyn Evidence) {
    EVIDENCE.install(evidence);
}

fn required() -> ApiResult<&'static dyn Evidence> {
    EVIDENCE
        .get()
        .ok_or_else(|| ApiError::Conflict("观测证据的接收方未登记，不能记录事件或故障".into()))
}

/// Adds one panel event to a server's timeline.
pub async fn record_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    kind: &str,
    detail: Value,
) -> ApiResult<()> {
    required()?.record_event_tx(tx, server, kind, detail).await
}

pub async fn open_incident_tx(
    tx: &mut Transaction<'_, Postgres>,
    report: IncidentReport<'_>,
) -> ApiResult<()> {
    required()?.open_incident_tx(tx, report).await
}

pub async fn recover_incident_tx(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
    evidence: Value,
    observed_at: i64,
    notify: bool,
) -> ApiResult<()> {
    required()?
        .recover_incident_tx(tx, key, evidence, observed_at, notify)
        .await
}

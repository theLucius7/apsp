//! Hooks through which the host reaches business plugins without naming them.
//!
//! The host owns the device channel and the shared diagnostic services, but the
//! messages it relays (usage batches, module manifests, configuration bundles)
//! belong to plugins. The panel binary installs one [`PanelPlugins`]
//! implementation at startup; the host only calls through this interface.

use std::{collections::BTreeMap, future::Future, pin::Pin, sync::OnceLock};

use apsp_protocol::{ModuleManifest, RuntimeCheckpointRequest, UsageBatch};
use axum::http::Method;
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    error::{ApiError, ApiResult},
};

pub type HookFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Runtime evidence supplied by plugins; silence does not establish idleness.
pub struct ActivityEvidence {
    pub configured: bool,
    pub last_positive_at: Option<i64>,
}

/// Authorization facts supplied by the plugin that owns a route.
#[derive(Clone, Copy, Default)]
pub struct RoutePolicy {
    pub independent_identity: bool,
    pub feature: Option<&'static str>,
    pub high_risk: bool,
    pub read: bool,
    pub scoped_objects: bool,
}

/// An external service whose expiry a plugin observes, listed in the costs and
/// renewals ledger for reference only: it has no price and is never renewed.
#[derive(Clone, serde::Serialize)]
pub struct RenewalReference {
    /// Kind of reference, stable for clients (for example `proxy_source`).
    pub kind: &'static str,
    pub id: i64,
    pub name: String,
    pub expires_at: Option<i64>,
    pub used_bytes: Option<i64>,
    pub total_bytes: Option<i64>,
    pub reported_at: Option<i64>,
    /// Hash path of the page that manages the referenced object.
    pub path: String,
    /// Capability the caller needs before the reference is shown.
    #[serde(skip)]
    pub capability: &'static str,
}

/// One category whose SQL is owned by a plugin; the host binds visibility first.
pub struct SearchSource {
    pub category: &'static str,
    pub capability: &'static str,
    pub global_only: bool,
    pub extra_ctes: &'static str,
    pub select: &'static str,
}

#[derive(Clone, Copy)]
pub struct RuntimeStepPolicy {
    pub version: &'static str,
    pub capability: &'static str,
}

/// A bounded TXT challenge; credentials remain references inside the provider.
pub struct DnsChallenge<'a> {
    pub rule: Uuid,
    pub challenge: Uuid,
    pub name: &'a str,
    pub value: &'a str,
    pub credential: Option<Uuid>,
}

/// The immutable deployment candidate and the actor bound to one task.
pub struct DeploymentIntent<'a> {
    pub server: i64,
    pub job: Uuid,
    pub actor: i64,
    pub candidate: &'a Value,
    pub expires: i64,
}

/// Device-channel hooks implemented by the assembled plugin set.
pub trait PanelPlugins: Send + Sync {
    /// Records one device usage batch; an error leaves the batch unacknowledged.
    fn ingest_usage<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        batch: UsageBatch,
    ) -> HookFuture<'a, anyhow::Result<()>>;

    /// Lists the modules a device must run, keyed by module identifier.
    fn manifest_modules<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        info: &'a serde_json::Value,
    ) -> HookFuture<'a, ApiResult<BTreeMap<String, ModuleManifest>>>;

    /// Returns the published configuration bundle at one revision.
    fn bundle<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        rev: i64,
    ) -> HookFuture<'a, ApiResult<String>>;

    /// Reports whether runtime traffic is configured and when it was last seen.
    fn runtime_activity_on<'a>(
        &'a self,
        connection: &'a mut sqlx::PgConnection,
        server_id: i64,
        checked_at: i64,
    ) -> HookFuture<'a, ApiResult<ActivityEvidence>>;
    fn route_policy(&self, path: &str, method: &Method) -> Option<RoutePolicy>;
    fn sensitive_key(&self, key: &str) -> bool;
    fn search_sources(&self) -> Vec<SearchSource>;
    fn worker_names(&self) -> &'static [&'static str];
    fn signed_tool_packages(&self, entries: &[crate::artifacts::ArtifactEntry]) -> Vec<Value>;
    fn audit_snapshot<'a>(
        &'a self,
        state: &'a AppState,
        path: &'a str,
    ) -> HookFuture<'a, ApiResult<Option<Value>>>;
    fn runtime_step_policy(&self, kind: &str) -> Option<RuntimeStepPolicy>;

    /// Records a device's admitted runtime session addresses. Devices report
    /// only when a plugin enabled session control in their configuration.
    fn ingest_runtime_sessions<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        report: apsp_protocol::RuntimeSessionReport,
    ) -> HookFuture<'a, anyhow::Result<()>> {
        let _ = (state, server_id, report);
        Box::pin(async { Ok(()) })
    }

    /// Expiries observed by plugins for the costs and renewals ledger.
    fn renewal_references<'a>(
        &'a self,
        state: &'a AppState,
    ) -> HookFuture<'a, ApiResult<Vec<RenewalReference>>> {
        let _ = state;
        Box::pin(async { Ok(Vec::new()) })
    }

    fn dns_present<'a>(
        &'a self,
        state: &'a AppState,
        challenge: DnsChallenge<'a>,
    ) -> HookFuture<'a, ApiResult<String>>;
    fn dns_cleanup<'a>(
        &'a self,
        state: &'a AppState,
        challenge: DnsChallenge<'a>,
        record: Option<&'a str>,
    ) -> HookFuture<'a, ApiResult<()>>;

    fn automation_candidate<'a, 't>(
        &'a self,
        state: &'a AppState,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
    ) -> HookFuture<'a, ApiResult<Value>>;
    fn enqueue_automation_deployment_tx<'a, 't>(
        &'a self,
        state: &'a AppState,
        tx: &'a mut Transaction<'t, Postgres>,
        intent: DeploymentIntent<'a>,
    ) -> HookFuture<'a, ApiResult<Uuid>>;
    fn cancel_automation_deployment_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        request: Uuid,
    ) -> HookFuture<'a, ApiResult<()>>;
    fn automation_dispatch_matches_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        request: Uuid,
    ) -> HookFuture<'a, ApiResult<bool>>;
    fn automation_deployment_receipt<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        request: Uuid,
    ) -> HookFuture<'a, ApiResult<Value>>;
    fn request_automation_deployment_checkpoint_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        original: Uuid,
    ) -> HookFuture<'a, ApiResult<RuntimeCheckpointRequest>>;
    fn reconcile_automation_deployment_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        original: Uuid,
        checkpoint: Uuid,
        evidence: &'a str,
    ) -> HookFuture<'a, ApiResult<Value>>;
}

static INSTALLED: OnceLock<&'static dyn PanelPlugins> = OnceLock::new();

/// Installs the plugin set once; later calls keep the first installation.
pub fn install(plugins: &'static dyn PanelPlugins) {
    let _ = INSTALLED.set(plugins);
}

fn installed() -> Option<&'static dyn PanelPlugins> {
    INSTALLED.get().copied()
}

pub async fn ingest_usage(
    state: &AppState,
    server_id: i64,
    batch: UsageBatch,
) -> anyhow::Result<()> {
    match installed() {
        Some(plugins) => plugins.ingest_usage(state, server_id, batch).await,
        None => anyhow::bail!("no plugin accepts usage batches"),
    }
}

pub async fn ingest_runtime_sessions(
    state: &AppState,
    server_id: i64,
    report: apsp_protocol::RuntimeSessionReport,
) -> anyhow::Result<()> {
    anyhow::ensure!(report.valid(), "invalid runtime session report");
    match installed() {
        Some(plugins) => {
            plugins
                .ingest_runtime_sessions(state, server_id, report)
                .await
        }
        None => Ok(()),
    }
}

pub async fn manifest_modules(
    state: &AppState,
    server_id: i64,
    info: &serde_json::Value,
) -> ApiResult<BTreeMap<String, ModuleManifest>> {
    match installed() {
        Some(plugins) => plugins.manifest_modules(state, server_id, info).await,
        None => Ok(BTreeMap::new()),
    }
}

pub async fn bundle(state: &AppState, server_id: i64, rev: i64) -> ApiResult<String> {
    match installed() {
        Some(plugins) => plugins.bundle(state, server_id, rev).await,
        None => Err(ApiError::NotFound),
    }
}

pub async fn runtime_activity_on(
    connection: &mut sqlx::PgConnection,
    server_id: i64,
    checked_at: i64,
) -> ApiResult<ActivityEvidence> {
    match installed() {
        Some(plugins) => {
            plugins
                .runtime_activity_on(connection, server_id, checked_at)
                .await
        }
        None => Err(ApiError::Conflict(
            "运行时活动证据提供方未登记，不能判断当前负载".into(),
        )),
    }
}

fn required() -> ApiResult<&'static dyn PanelPlugins> {
    installed()
        .ok_or_else(|| ApiError::Conflict("执行器提供方未登记，无法确认能力或安全执行".into()))
}

pub fn route_policy(path: &str, method: &Method) -> Option<RoutePolicy> {
    installed().and_then(|plugins| plugins.route_policy(path, method))
}
pub fn sensitive_key(key: &str) -> bool {
    installed().is_some_and(|plugins| plugins.sensitive_key(key))
}
pub fn search_sources() -> Vec<SearchSource> {
    installed()
        .map(|plugins| plugins.search_sources())
        .unwrap_or_default()
}
pub fn worker_names() -> &'static [&'static str] {
    installed()
        .map(|plugins| plugins.worker_names())
        .unwrap_or_default()
}
pub fn signed_tool_packages(entries: &[crate::artifacts::ArtifactEntry]) -> Vec<Value> {
    installed()
        .map(|plugins| plugins.signed_tool_packages(entries))
        .unwrap_or_default()
}
pub async fn renewal_references(state: &AppState) -> ApiResult<Vec<RenewalReference>> {
    match installed() {
        Some(plugins) => plugins.renewal_references(state).await,
        None => Ok(Vec::new()),
    }
}
pub async fn audit_snapshot(state: &AppState, path: &str) -> ApiResult<Option<Value>> {
    match installed() {
        Some(plugins) => plugins.audit_snapshot(state, path).await,
        None => Ok(None),
    }
}
pub fn runtime_step_policy(kind: &str) -> Option<RuntimeStepPolicy> {
    installed().and_then(|plugins| plugins.runtime_step_policy(kind))
}
pub async fn dns_present(
    state: &AppState,
    rule: Uuid,
    challenge: Uuid,
    name: &str,
    value: &str,
    credential: Option<Uuid>,
) -> ApiResult<String> {
    required()?
        .dns_present(
            state,
            DnsChallenge {
                rule,
                challenge,
                name,
                value,
                credential,
            },
        )
        .await
}
pub async fn dns_cleanup(
    state: &AppState,
    rule: Uuid,
    challenge: Uuid,
    name: &str,
    value: &str,
    record: Option<&str>,
    credential: Option<Uuid>,
) -> ApiResult<()> {
    required()?
        .dns_cleanup(
            state,
            DnsChallenge {
                rule,
                challenge,
                name,
                value,
                credential,
            },
            record,
        )
        .await
}
pub async fn automation_candidate(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<Value> {
    required()?.automation_candidate(state, tx, server).await
}
pub async fn enqueue_automation_deployment_tx(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    job: Uuid,
    actor: i64,
    candidate: &Value,
    expires: i64,
) -> ApiResult<Uuid> {
    required()?
        .enqueue_automation_deployment_tx(
            state,
            tx,
            DeploymentIntent {
                server,
                job,
                actor,
                candidate,
                expires,
            },
        )
        .await
}
pub async fn cancel_automation_deployment_tx(
    tx: &mut Transaction<'_, Postgres>,
    request: Uuid,
) -> ApiResult<()> {
    required()?
        .cancel_automation_deployment_tx(tx, request)
        .await
}
pub async fn automation_dispatch_matches_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    request: Uuid,
) -> ApiResult<bool> {
    required()?
        .automation_dispatch_matches_tx(tx, server, request)
        .await
}
pub async fn automation_deployment_receipt(
    tx: &mut Transaction<'_, Postgres>,
    request: Uuid,
) -> ApiResult<Value> {
    required()?.automation_deployment_receipt(tx, request).await
}
pub async fn request_automation_deployment_checkpoint_tx(
    tx: &mut Transaction<'_, Postgres>,
    original: Uuid,
) -> ApiResult<RuntimeCheckpointRequest> {
    required()?
        .request_automation_deployment_checkpoint_tx(tx, original)
        .await
}
pub async fn reconcile_automation_deployment_tx(
    tx: &mut Transaction<'_, Postgres>,
    original: Uuid,
    checkpoint: Uuid,
    evidence: &str,
) -> ApiResult<Value> {
    required()?
        .reconcile_automation_deployment_tx(tx, original, checkpoint, evidence)
        .await
}

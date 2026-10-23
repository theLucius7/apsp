//! Compiled plugin set: the only place that names concrete business plugins.

pub use apsp_cloud_api as cloud_api;
pub use apsp_panel_host::plugin_api::ActivityEvidence;
pub use apsp_plugin_alicloud as alicloud;
pub use apsp_plugin_ddns as ddns;
pub use apsp_plugin_singbox as singbox;

use std::collections::BTreeMap;

use apsp_panel_host::{
    AppState,
    error::ApiResult,
    plugin_api::{
        self, DeploymentIntent, DnsChallenge, HookFuture, PanelPlugins, RoutePolicy,
        RuntimeStepPolicy, SearchSource,
    },
};
use apsp_protocol::{ModuleManifest, RuntimeCheckpointRequest, UsageBatch};
use axum::Router;
use axum::http::Method;
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

struct Compiled;

impl PanelPlugins for Compiled {
    fn ingest_usage<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        batch: UsageBatch,
    ) -> HookFuture<'a, anyhow::Result<()>> {
        Box::pin(singbox::usage::ingest(state, server_id, batch))
    }

    fn manifest_modules<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        info: &'a serde_json::Value,
    ) -> HookFuture<'a, ApiResult<BTreeMap<String, ModuleManifest>>> {
        Box::pin(async move {
            let mut modules = BTreeMap::new();
            if let Some(module) = singbox::agent::manifest_module(state, server_id, info).await? {
                modules.insert("singbox".into(), module);
            }
            Ok(modules)
        })
    }

    fn bundle<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        rev: i64,
    ) -> HookFuture<'a, ApiResult<String>> {
        Box::pin(singbox::agent::bundle(state, server_id, rev))
    }

    fn runtime_activity_on<'a>(
        &'a self,
        connection: &'a mut sqlx::PgConnection,
        server_id: i64,
        checked_at: i64,
    ) -> HookFuture<'a, ApiResult<ActivityEvidence>> {
        Box::pin(singbox::runtime_activity_on(
            connection, server_id, checked_at,
        ))
    }
    fn route_policy(&self, path: &str, method: &Method) -> Option<RoutePolicy> {
        crate::integration_policy::route_policy(path, method)
    }
    fn sensitive_key(&self, key: &str) -> bool {
        crate::integration_policy::sensitive_key(key)
    }
    fn search_sources(&self) -> Vec<SearchSource> {
        crate::integration_policy::search_sources()
    }
    fn worker_names(&self) -> &'static [&'static str] {
        crate::integration_policy::worker_names()
    }
    fn signed_tool_packages(
        &self,
        entries: &[apsp_panel_host::artifacts::ArtifactEntry],
    ) -> Vec<Value> {
        crate::integration_policy::signed_tool_packages(entries)
    }
    fn audit_snapshot<'a>(
        &'a self,
        state: &'a AppState,
        path: &'a str,
    ) -> HookFuture<'a, ApiResult<Option<Value>>> {
        Box::pin(crate::integration_policy::audit_snapshot(state, path))
    }
    fn runtime_step_policy(&self, kind: &str) -> Option<RuntimeStepPolicy> {
        crate::integration_policy::runtime_step_policy(kind)
    }
    fn dns_present<'a>(
        &'a self,
        state: &'a AppState,
        input: DnsChallenge<'a>,
    ) -> HookFuture<'a, ApiResult<String>> {
        Box::pin(ddns::dns01::present(
            state,
            input.rule,
            input.challenge,
            input.name,
            input.value,
            input.credential,
        ))
    }
    fn dns_cleanup<'a>(
        &'a self,
        state: &'a AppState,
        input: DnsChallenge<'a>,
        record: Option<&'a str>,
    ) -> HookFuture<'a, ApiResult<()>> {
        Box::pin(ddns::dns01::cleanup(
            state,
            input.rule,
            input.challenge,
            input.name,
            input.value,
            record,
            input.credential,
        ))
    }
    fn automation_candidate<'a, 't>(
        &'a self,
        state: &'a AppState,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
    ) -> HookFuture<'a, ApiResult<Value>> {
        Box::pin(singbox::operations_workflows::automation_candidate(
            state, tx, server,
        ))
    }
    fn enqueue_automation_deployment_tx<'a, 't>(
        &'a self,
        state: &'a AppState,
        tx: &'a mut Transaction<'t, Postgres>,
        intent: DeploymentIntent<'a>,
    ) -> HookFuture<'a, ApiResult<Uuid>> {
        Box::pin(
            singbox::operations_workflows::enqueue_automation_deployment_tx(
                state,
                tx,
                intent.server,
                intent.job,
                intent.actor,
                intent.candidate,
                intent.expires,
            ),
        )
    }
    fn cancel_automation_deployment_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        request: Uuid,
    ) -> HookFuture<'a, ApiResult<()>> {
        Box::pin(singbox::operations_workflows::cancel_automation_deployment_tx(tx, request))
    }
    fn automation_dispatch_matches_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        request: Uuid,
    ) -> HookFuture<'a, ApiResult<bool>> {
        Box::pin(singbox::operations_workflows::automation_dispatch_matches_tx(tx, server, request))
    }
    fn automation_deployment_receipt<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        request: Uuid,
    ) -> HookFuture<'a, ApiResult<Value>> {
        Box::pin(singbox::operations_workflows::automation_deployment_receipt(tx, request))
    }
    fn request_automation_deployment_checkpoint_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        original: Uuid,
    ) -> HookFuture<'a, ApiResult<RuntimeCheckpointRequest>> {
        Box::pin(
            singbox::operations_workflows::request_automation_deployment_checkpoint_tx(
                tx, original,
            ),
        )
    }
    fn reconcile_automation_deployment_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        original: Uuid,
        checkpoint: Uuid,
        evidence: &'a str,
    ) -> HookFuture<'a, ApiResult<Value>> {
        Box::pin(
            singbox::operations_workflows::reconcile_automation_deployment_tx(
                tx, original, checkpoint, evidence,
            ),
        )
    }
}

static COMPILED: Compiled = Compiled;

/// Registers the compiled plugins with the host; repeated calls are harmless.
pub fn install() {
    plugin_api::install(&COMPILED);
}

pub fn router() -> Router<AppState> {
    install();
    singbox::router()
        .merge(ddns::routes())
        .merge(alicloud::routes())
        .merge(alicloud::security_groups_routes())
}

pub async fn run(state: AppState) {
    use apsp_panel_host::maintenance::supervise;
    install();
    // Supervise each plugin separately: one plugin's panic must not stop the others.
    let pool = state.pool.clone();
    tokio::join!(
        supervise("sing-box", move || singbox::run(state.clone())),
        supervise("ddns", {
            let pool = pool.clone();
            move || ddns::run(pool.clone())
        }),
        supervise("alicloud", move || alicloud::run(pool.clone()))
    );
}

// Direct entry points kept for embedders of the previous public API.

pub async fn runtime_activity_on(
    connection: &mut sqlx::PgConnection,
    server_id: i64,
    checked_at: i64,
) -> ApiResult<ActivityEvidence> {
    singbox::runtime_activity_on(connection, server_id, checked_at).await
}

pub async fn ingest_usage(
    state: &AppState,
    server_id: i64,
    batch: UsageBatch,
) -> anyhow::Result<()> {
    singbox::usage::ingest(state, server_id, batch).await
}

pub async fn manifest_modules(
    state: &AppState,
    server_id: i64,
    info: &serde_json::Value,
) -> ApiResult<BTreeMap<String, ModuleManifest>> {
    COMPILED.manifest_modules(state, server_id, info).await
}

pub async fn bundle(state: &AppState, server_id: i64, rev: i64) -> ApiResult<String> {
    singbox::agent::bundle(state, server_id, rev).await
}

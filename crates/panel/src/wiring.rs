//! Connects the typed interfaces at startup (ADR 0128): device reports, the traffic
//! runtime's configuration, administration contributions, deployment automation,
//! DNS-01 records, runtime activity and renewal references. This is the only place
//! that names which owner answers each interface.

use std::collections::BTreeMap;

use apsp_costs::renewals::{self, RenewalReference, RenewalSources};
use apsp_naming as naming;
use apsp_panel_host::{
    AppState,
    agent_api::{self, DeviceReports},
    artifacts::ArtifactEntry,
    control_center::contributions::{self, Contributions, RoutePolicy, SearchSource},
    device_config::{self, ConfigSource},
    diagnostics::{
        self,
        activity::{self, ActivityEvidence, RuntimeActivity},
    },
    error::ApiResult,
    network_configuration::dns_challenges::{self, Dns01, DnsChallenge},
    operations::deployment_automation::{
        self, DeploymentAutomation, DeploymentIntent, RuntimeStepPolicy,
    },
    ports::BoxFuture,
    retirement, runtime_control, servers, telemetry,
};
use apsp_plugin_singbox as singbox;
use apsp_protocol::{
    AppliedRevisions, ApplyResult, DiagnosticCancelResult, Metrics, ModuleManifest,
    RetirementResult, RuntimeCheckpointRequest, RuntimeCheckpointResult, RuntimeControlAck,
    RuntimePathProbeResult, RuntimeRecoveryBarrierResult, RuntimeSessionReport, StaticInfo,
    UsageBatch,
};
use axum::http::Method;
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

struct Wiring;

static WIRING: Wiring = Wiring;

/// Connects every interface; repeated calls keep the first connection.
pub fn install() {
    agent_api::install(&WIRING);
    device_config::install(&WIRING);
    contributions::install(&WIRING);
    deployment_automation::install(&WIRING);
    dns_challenges::install(&WIRING);
    activity::install(&WIRING);
    renewals::install(&WIRING);
}

/// The modules each device runs; the traffic runtime is the only module today.
pub async fn manifest_modules(
    state: &AppState,
    server_id: i64,
    info: &Value,
) -> ApiResult<BTreeMap<String, ModuleManifest>> {
    let mut modules = BTreeMap::new();
    if let Some(module) = singbox::agent::manifest_module(state, server_id, info).await? {
        modules.insert("singbox".into(), module);
    }
    Ok(modules)
}

pub async fn runtime_activity_on(
    connection: &mut sqlx::PgConnection,
    server_id: i64,
    checked_at: i64,
) -> ApiResult<ActivityEvidence> {
    let evidence = singbox::runtime_activity_on(connection, server_id, checked_at).await?;
    Ok(ActivityEvidence {
        configured: evidence.configured,
        last_positive_at: evidence.last_positive_at,
    })
}

impl DeviceReports for Wiring {
    fn hello<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        capabilities: Vec<String>,
        applied: AppliedRevisions,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            runtime_control::register_capabilities(state, server_id, &capabilities).await?;
            servers::record_capabilities(state, server_id, &capabilities).await?;
            device_config::reconcile_applied(state, server_id, applied).await?;
            runtime_control::replay_pending(state, server_id).await
        })
    }

    fn heartbeat<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        applied: AppliedRevisions,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            servers::record_heartbeat(state, server_id).await?;
            device_config::reconcile_applied(state, server_id, applied).await?;
            runtime_control::replay_pending(state, server_id).await
        })
    }

    fn static_info<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        info: StaticInfo,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(servers::record_static_info(state, server_id, info))
    }

    fn metrics<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        metrics: Metrics,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(telemetry::record_metrics(state, server_id, metrics))
    }

    fn apply_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: ApplyResult,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(device_config::record_apply_result(state, server_id, result))
    }

    fn checkpoint_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: RuntimeCheckpointResult,
    ) -> BoxFuture<'a, anyhow::Result<RuntimeControlAck>> {
        Box::pin(runtime_control::record_checkpoint_result(
            state, server_id, result,
        ))
    }

    fn path_probe_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: RuntimePathProbeResult,
    ) -> BoxFuture<'a, anyhow::Result<RuntimeControlAck>> {
        Box::pin(runtime_control::record_path_probe_result(
            state, server_id, result,
        ))
    }

    fn barrier_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: RuntimeRecoveryBarrierResult,
    ) -> BoxFuture<'a, anyhow::Result<RuntimeControlAck>> {
        Box::pin(runtime_control::record_barrier_result(
            state, server_id, result,
        ))
    }

    fn diagnostic_cancel_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: DiagnosticCancelResult,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            diagnostics::cancellation::record_result(state, server_id, result).await?;
            Ok(())
        })
    }

    fn usage_batch<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        batch: UsageBatch,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(singbox::usage::ingest(state, server_id, batch))
    }

    fn runtime_sessions<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        report: RuntimeSessionReport,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(singbox::ingest_runtime_sessions(state, server_id, report))
    }

    fn retirement_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: RetirementResult,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(retirement::record_result(state, server_id, result))
    }
}

impl ConfigSource for Wiring {
    fn manifest_modules<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        info: &'a Value,
    ) -> BoxFuture<'a, ApiResult<BTreeMap<String, ModuleManifest>>> {
        Box::pin(manifest_modules(state, server_id, info))
    }

    fn bundle<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        rev: i64,
    ) -> BoxFuture<'a, ApiResult<String>> {
        Box::pin(singbox::agent::bundle(state, server_id, rev))
    }
}

impl Contributions for Wiring {
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

    fn signed_tool_packages(&self, entries: &[ArtifactEntry]) -> Vec<Value> {
        crate::integration_policy::signed_tool_packages(entries)
    }

    fn audit_snapshot<'a>(
        &'a self,
        state: &'a AppState,
        path: &'a str,
    ) -> BoxFuture<'a, ApiResult<Option<Value>>> {
        Box::pin(crate::integration_policy::audit_snapshot(state, path))
    }
}

impl DeploymentAutomation for Wiring {
    fn runtime_step_policy(&self, kind: &str) -> Option<RuntimeStepPolicy> {
        crate::integration_policy::runtime_step_policy(kind)
    }

    fn candidate<'a, 't>(
        &'a self,
        state: &'a AppState,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
    ) -> BoxFuture<'a, ApiResult<Value>> {
        Box::pin(singbox::operations_workflows::automation_candidate(
            state, tx, server,
        ))
    }

    fn enqueue_tx<'a, 't>(
        &'a self,
        state: &'a AppState,
        tx: &'a mut Transaction<'t, Postgres>,
        intent: DeploymentIntent<'a>,
    ) -> BoxFuture<'a, ApiResult<Uuid>> {
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

    fn cancel_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        request: Uuid,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(singbox::operations_workflows::cancel_automation_deployment_tx(tx, request))
    }

    fn dispatch_matches_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        request: Uuid,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(singbox::operations_workflows::automation_dispatch_matches_tx(tx, server, request))
    }

    fn receipt<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        request: Uuid,
    ) -> BoxFuture<'a, ApiResult<Value>> {
        Box::pin(singbox::operations_workflows::automation_deployment_receipt(tx, request))
    }

    fn request_checkpoint_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        original: Uuid,
    ) -> BoxFuture<'a, ApiResult<RuntimeCheckpointRequest>> {
        Box::pin(
            singbox::operations_workflows::request_automation_deployment_checkpoint_tx(
                tx, original,
            ),
        )
    }

    fn reconcile_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        original: Uuid,
        checkpoint: Uuid,
        evidence: &'a str,
    ) -> BoxFuture<'a, ApiResult<Value>> {
        Box::pin(
            singbox::operations_workflows::reconcile_automation_deployment_tx(
                tx, original, checkpoint, evidence,
            ),
        )
    }
}

impl Dns01 for Wiring {
    fn present<'a>(
        &'a self,
        state: &'a AppState,
        challenge: DnsChallenge<'a>,
    ) -> BoxFuture<'a, ApiResult<String>> {
        Box::pin(naming::dns01::present(
            state,
            challenge.rule,
            challenge.challenge,
            challenge.name,
            challenge.value,
            challenge.credential,
        ))
    }

    fn cleanup<'a>(
        &'a self,
        state: &'a AppState,
        challenge: DnsChallenge<'a>,
        record: Option<&'a str>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(naming::dns01::cleanup(
            state,
            challenge.rule,
            challenge.challenge,
            challenge.name,
            challenge.value,
            record,
            challenge.credential,
        ))
    }
}

impl RuntimeActivity for Wiring {
    fn activity_on<'a>(
        &'a self,
        connection: &'a mut sqlx::PgConnection,
        server_id: i64,
        checked_at: i64,
    ) -> BoxFuture<'a, ApiResult<ActivityEvidence>> {
        Box::pin(runtime_activity_on(connection, server_id, checked_at))
    }
}

impl RenewalSources for Wiring {
    fn renewal_references<'a>(
        &'a self,
        state: &'a AppState,
    ) -> BoxFuture<'a, ApiResult<Vec<RenewalReference>>> {
        Box::pin(async move {
            Ok(singbox::subscription_sources::renewal_references(state)
                .await?
                .into_iter()
                .map(|source| RenewalReference {
                    kind: source.kind,
                    id: source.id,
                    name: source.name,
                    expires_at: source.expires_at,
                    used_bytes: source.used_bytes,
                    total_bytes: source.total_bytes,
                    reported_at: source.reported_at,
                    path: source.path,
                    capability: source.capability,
                })
                .collect())
        })
    }
}

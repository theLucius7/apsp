//! Connects the typed interfaces at startup (ADR 0128): device reports, administration
//! contributions, device work, observability evidence, server observation, the member
//! portal's host network, the proxy users traffic compiles in, China reachability for the
//! Nexus cutover, the dashboard's exchange rates and DNS-01 records. This is the
//! only place that names which owner answers each interface.

use apsp_access as access;
use apsp_naming as naming;
use apsp_observability as observability;
use apsp_operations::{fleet, runtime_operations};
use apsp_panel_host::{
    AppState,
    agent_api::{self, DeviceReports},
    artifacts::ArtifactEntry,
    control_center::contributions::{self, Contributions, RoutePolicy, SearchSource},
    device_work::{self, DeviceWork},
    error::ApiResult,
    evidence::{self, Evidence, IncidentReport},
    network_configuration::dns_challenges::{self, Dns01, DnsChallenge},
    ports::BoxFuture,
    retirement,
    server_observation::{self, ServerObservation},
    servers::{self, Server},
};
use apsp_protocol::{
    AppliedRevisions, ApplyResult, DiagnosticCancelResult, Metrics, ProbeSpec, RetirementResult,
    RuntimeCheckpointResult, RuntimeControlAck, RuntimeOperationRequest, RuntimePathProbeResult,
    RuntimeRecoveryBarrierResult, RuntimeSessionReport, StaticInfo, UsageBatch, fleet::Operation,
};
use apsp_traffic as traffic;
use axum::http::Method;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

struct Wiring;

static WIRING: Wiring = Wiring;

/// Connects every interface; repeated calls keep the first connection.
pub fn install() {
    agent_api::install(&WIRING);
    contributions::install(&WIRING);
    device_work::install(&WIRING);
    evidence::install(&WIRING);
    server_observation::install(&WIRING);
    access::install_host_network(&WIRING);
    traffic::users::install(&WIRING);
    traffic::china_reachability::install(&WIRING);
    apsp_plugin_alicloud::operations::install();
    observability::dashboard::install(&WIRING);
    dns_challenges::install(&WIRING);
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
            traffic::runtime_control::register_capabilities(state, server_id, &capabilities)
                .await?;
            servers::record_capabilities(state, server_id, &capabilities).await?;
            traffic::device_config::reconcile_applied(state, server_id, applied).await?;
            traffic::runtime_control::replay_pending(state, server_id).await
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
            traffic::device_config::reconcile_applied(state, server_id, applied).await?;
            traffic::runtime_control::replay_pending(state, server_id).await
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
        Box::pin(observability::telemetry::record_metrics(
            state, server_id, metrics,
        ))
    }

    fn apply_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: ApplyResult,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(traffic::device_config::record_apply_result(
            state, server_id, result,
        ))
    }

    fn checkpoint_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: RuntimeCheckpointResult,
    ) -> BoxFuture<'a, anyhow::Result<RuntimeControlAck>> {
        Box::pin(traffic::runtime_control::record_checkpoint_result(
            state, server_id, result,
        ))
    }

    fn path_probe_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: RuntimePathProbeResult,
    ) -> BoxFuture<'a, anyhow::Result<RuntimeControlAck>> {
        Box::pin(traffic::runtime_control::record_path_probe_result(
            state, server_id, result,
        ))
    }

    fn barrier_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: RuntimeRecoveryBarrierResult,
    ) -> BoxFuture<'a, anyhow::Result<RuntimeControlAck>> {
        Box::pin(traffic::runtime_control::record_barrier_result(
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
            observability::diagnostics::cancellation::record_result(state, server_id, result)
                .await?;
            Ok(())
        })
    }

    fn usage_batch<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        batch: UsageBatch,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(access::usage::ingest(state, server_id, batch))
    }

    fn runtime_sessions<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        report: RuntimeSessionReport,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(access::nexus::sessions::ingest(state, server_id, report))
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

impl DeviceWork for Wiring {
    fn ensure_accepts_tasks_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(fleet::ensure_accepts_tasks_tx(tx, server))
    }

    fn submit_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        actor: i64,
        operation: Operation,
    ) -> BoxFuture<'a, ApiResult<Uuid>> {
        Box::pin(fleet::enqueue_for_actor_tx(tx, server, actor, operation))
    }

    fn submit_observation_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        actor: i64,
        operation: Operation,
    ) -> BoxFuture<'a, ApiResult<Uuid>> {
        Box::pin(fleet::enqueue_readonly_for_actor_tx(
            tx, server, actor, operation,
        ))
    }

    fn runtime_busy_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        now: i64,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(runtime_operations::busy(tx, server, now))
    }

    fn submit_runtime_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        request: &'a RuntimeOperationRequest,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(runtime_operations::enqueue(
            tx,
            server,
            &request.module,
            request,
        ))
    }

    fn latest_runtime_for_principal_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        module: &'a str,
        pattern: &'a str,
    ) -> BoxFuture<'a, ApiResult<Vec<Value>>> {
        Box::pin(runtime_operations::latest_for_principal(
            tx, server, module, pattern,
        ))
    }
}

impl observability::dashboard::ExchangeRates for Wiring {
    fn current<'a>(&'a self, pool: &'a PgPool) -> BoxFuture<'a, ApiResult<Value>> {
        Box::pin(async move {
            Ok(
                serde_json::to_value(apsp_costs::exchange::current(pool).await?)
                    .map_err(anyhow::Error::from)?,
            )
        })
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

impl Evidence for Wiring {
    fn record_event_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        kind: &'a str,
        detail: Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(observability::server_monitoring::record_event(
            tx, server, kind, detail,
        ))
    }

    fn open_incident_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        report: IncidentReport<'a>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(observability::incidents::open(
            tx,
            report.key,
            report.server,
            report.title,
            report.severity,
            report.evidence,
            report.observed_at,
            None,
        ))
    }

    fn recover_incident_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        key: &'a str,
        evidence: Value,
        observed_at: i64,
        notify: bool,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(observability::incidents::recover(
            tx,
            key,
            evidence,
            observed_at,
            notify,
        ))
    }
}

impl ServerObservation for Wiring {
    fn attach_traffic<'a>(
        &'a self,
        pool: &'a PgPool,
        servers: &'a mut [Server],
        now: i64,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(observability::server_traffic::attach(pool, servers, now))
    }

    fn attach_live<'a>(
        &'a self,
        state: &'a AppState,
        servers: &'a mut [Server],
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(observability::telemetry::attach_live(state, servers))
    }

    fn prepare_probe(&self, spec: &mut ProbeSpec) -> ApiResult<()> {
        observability::probes::prepare_write(spec)
    }

    fn check_alert_rule(&self, rule: &Value) -> ApiResult<()> {
        observability::alerts::rules::check_template(rule)
    }

    fn lock_latency_tasks_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { Ok(observability::latency_tasks::lock(tx).await?) })
    }

    fn assign_default_latency_tasks_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(observability::latency_tasks::assign_defaults(tx, server))
    }
}

impl access::HostNetwork for Wiring {
    fn member_network<'a>(
        &'a self,
        state: &'a AppState,
        server: i64,
        static_info: &'a Value,
    ) -> BoxFuture<'a, ApiResult<Value>> {
        Box::pin(observability::ip_quality::member_network(
            state,
            server,
            static_info,
        ))
    }
}

impl traffic::users::Users for Wiring {
    fn apply_limits<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        config: &'a str,
    ) -> BoxFuture<'a, anyhow::Result<String>> {
        Box::pin(access::user_limits::compile(tx, config))
    }

    fn nexus_device_limits<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        now: i64,
    ) -> BoxFuture<'a, anyhow::Result<(i64, bool)>> {
        Box::pin(access::nexus::sessions::limits_on_host(tx, server, now))
    }

    fn nexus_bridge_synced<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        since: i64,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(access::nexus::bridge::synced_since(tx, since).await?) })
    }

    fn remove_node_accesses<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        node: i64,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(access::accesses::remove_node(tx, node))
    }

    fn leave_policy_groups<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        nodes: &'a [i64],
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(access::policies::leave_groups(tx, nodes))
    }

    fn follow_legacy_groups<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        node: i64,
        legacy_groups: &'a [i64],
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(access::policies::follow_legacy_groups(
            tx,
            node,
            legacy_groups,
        ))
    }

    fn record_event<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        administrator: Option<i64>,
        user: Option<i64>,
        action: &'a str,
        detail: Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(access::operations_workflows::event(
            tx,
            administrator,
            user,
            action,
            detail,
        ))
    }
}

impl traffic::china_reachability::ChinaReachability for Wiring {
    fn nexus_node<'a>(
        &'a self,
        pool: &'a PgPool,
        node: i64,
    ) -> BoxFuture<'a, ApiResult<Option<traffic::china_reachability::Measurement>>> {
        Box::pin(async move {
            Ok(observability::reachability::latest_node_state(
                pool,
                observability::reachability::NodeReference::Nexus(node),
            )
            .await?
            .map(|state| traffic::china_reachability::Measurement {
                blocked: state.blocked,
                checked_at: state.checked_at,
            }))
        })
    }
}

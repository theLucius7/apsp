mod budget;
mod sections;
use crate::{
    Config, SharedState,
    artifacts::{PanelClient, safe_component},
};
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::{
    Descriptor, DiagnosticAdapter, DiagnosticSpec, JobStatus, Privileged, ServiceJob,
    ServiceManager,
};
use apsp_protocol::release::{ReleaseError, TrustedKeys};
use apsp_protocol::{DiagnosticJob, DiagnosticReport, DiagnosticStatus, DiagnosticUpdate};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    future::Future,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
use uuid::Uuid;

const ACTIVE: &str = "diagnostics:active";
const OUTBOX: &str = "diagnostics:outbox";
const MAX_REPORT: usize = 512 * 1024;

pub mod cancellation;
mod environment;
mod finalization;
mod monitoring;
mod observation;
mod safety;

#[derive(Clone, Serialize, Deserialize)]
enum Checkpoint {
    Preparing(DiagnosticJob),
    Started {
        spec: DiagnosticSpec,
        service: ServiceJob,
        started_at: u64,
        plugin: String,
        #[serde(default)]
        start_error: Option<String>,
        #[serde(default)]
        expires_at: Option<i64>,
        #[serde(default)]
        protection_stop_reason: Option<String>,
        #[serde(default)]
        environment: Option<environment::ExecutionEnvironment>,
        #[serde(default)]
        terminal_update: Option<Box<DiagnosticUpdate>>,
        #[serde(default)]
        cleanup_error: Option<String>,
    },
}

pub struct DiagnosticWorker {
    config: Config,
    state: SharedState,
    adapters: BTreeMap<String, Arc<dyn DiagnosticAdapter>>,
    privileged: Arc<dyn Privileged>,
    services: Arc<dyn ServiceManager>,
    keys: std::result::Result<TrustedKeys, ReleaseError>,
    cancellations: Option<Arc<cancellation::CancellationControl>>,
    retirement: Option<Arc<crate::retirement::Retirement>>,
}

impl DiagnosticWorker {
    pub fn new(
        config: Config,
        state: SharedState,
        adapters: Vec<Arc<dyn DiagnosticAdapter>>,
        privileged: Arc<dyn Privileged>,
        services: Arc<dyn ServiceManager>,
    ) -> Result<Self> {
        let mut registered = BTreeMap::new();
        for adapter in adapters {
            let descriptor = adapter.describe();
            ensure!(
                safe_component(&descriptor.plugin_name) && safe_component(&descriptor.binary_name),
                "invalid diagnostic plugin descriptor"
            );
            ensure!(
                registered.insert(descriptor.plugin_name, adapter).is_none(),
                "duplicate diagnostic plugin"
            );
        }
        Ok(Self {
            config,
            state,
            adapters: registered,
            privileged,
            services,
            keys: TrustedKeys::compiled(),
            cancellations: None,
            retirement: None,
        })
    }

    pub fn with_cancellations(mut self, control: Arc<cancellation::CancellationControl>) -> Self {
        self.cancellations = Some(control);
        self
    }

    /// Supplies roots already trusted by an embedding caller, including test fixtures.
    pub fn with_trusted_keys(mut self, keys: TrustedKeys) -> Self {
        self.keys = Ok(keys);
        self
    }

    fn retiring(&self) -> bool {
        self.retirement
            .as_ref()
            .is_some_and(|retirement| retirement.requested())
    }

    fn read<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
            .get_json(key)
    }

    async fn bounded<T>(&self, future: impl Future<Output = Result<T>>) -> Result<T> {
        tokio::time::timeout(
            Duration::from_secs(self.config.operation_timeout_secs),
            future,
        )
        .await
        .context("diagnostic operation timed out")?
    }

    fn save(&self, checkpoint: &Checkpoint) -> Result<()> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
            .set_json(ACTIVE, &Some(checkpoint))
    }

    fn active(&self) -> Result<Option<Checkpoint>> {
        Ok(self.read::<Option<Checkpoint>>(ACTIVE)?.flatten())
    }

    fn save_if_owned(&self, checkpoint: &Checkpoint) -> Result<bool> {
        let Checkpoint::Started {
            spec,
            terminal_update,
            ..
        } = checkpoint
        else {
            anyhow::bail!("only started diagnostic updates require ownership");
        };
        let id = Uuid::parse_str(&spec.id)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        if state
            .get_json::<bool>(&format!("diagnostics:done:{id}"))?
            .unwrap_or(false)
        {
            return Ok(false);
        }
        let active = state.get_json::<Option<Checkpoint>>(ACTIVE)?.flatten();
        let Some(Checkpoint::Started {
            spec: known,
            terminal_update: frozen,
            ..
        }) = active
        else {
            return Ok(false);
        };
        if known.id != spec.id || (frozen.is_some() && frozen != *terminal_update) {
            return Ok(false);
        }
        state.set_json(ACTIVE, &Some(checkpoint))?;
        Ok(true)
    }

    fn finish(&self, update: DiagnosticUpdate) -> Result<()> {
        self.finish_result(update, false)
    }

    fn finish_owned(&self, update: DiagnosticUpdate) -> Result<()> {
        self.finish_result(update, true)
    }

    fn finish_result(&self, update: DiagnosticUpdate, require_owned: bool) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        if state
            .get_json::<bool>(&format!("diagnostics:done:{}", update.id))?
            .unwrap_or(false)
        {
            return Ok(());
        }
        let active: Option<Checkpoint> = state.get_json::<Option<Checkpoint>>(ACTIVE)?.flatten();
        if require_owned
            && !matches!(&active,
            Some(Checkpoint::Started { spec, terminal_update: Some(saved), .. })
                if spec.id == update.id.to_string() && saved.as_ref() == &update)
        {
            return Ok(());
        }
        if let Some(active) = active {
            let active_id = match active {
                Checkpoint::Preparing(job) => job.id,
                Checkpoint::Started { spec, .. } => Uuid::parse_str(&spec.id)?,
            };
            ensure!(
                active_id == update.id,
                "terminal result differs from the active diagnostic"
            );
        }
        if self.cancellations.is_some() {
            let requests: Vec<apsp_protocol::DiagnosticCancelRequest> =
                state.get_json(cancellation::PENDING)?.unwrap_or_default();
            if requests.iter().any(|request| request.job.id == update.id) {
                // The same SQLite lock serializes this decision with cancellation.
                return Ok(());
            }
        }
        let mut outbox: Vec<DiagnosticUpdate> = state.get_json(OUTBOX)?.unwrap_or_default();
        if !outbox.iter().any(|saved| saved.id == update.id) {
            outbox.push(update.clone());
        }
        state.set_json_batch(&[
            (ACTIVE.into(), serde_json::Value::Null),
            (OUTBOX.into(), serde_json::to_value(outbox)?),
            (
                format!("diagnostics:done:{}", update.id),
                serde_json::Value::Bool(true),
            ),
        ])
    }

    async fn flush(&self, client: &PanelClient) -> Result<()> {
        // A chapter upload failure must not hide an execution failure or completion.
        let chapters_result = self.flush_sections(client).await;
        let pending: Vec<DiagnosticUpdate> = self.read(OUTBOX)?.unwrap_or_default();
        for update in pending {
            self.bounded(client.diagnostic_update(&update)).await?;
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
            let mut outbox: Vec<DiagnosticUpdate> = state.get_json(OUTBOX)?.unwrap_or_default();
            outbox.retain(|saved| saved.id != update.id);
            state.set_json(OUTBOX, &outbox)?;
        }
        chapters_result
    }

    fn accept(&self, jobs: Vec<DiagnosticJob>) -> Result<()> {
        if self.active()?.is_some() {
            return Ok(());
        }
        for job in jobs {
            if self
                .read::<bool>(&format!("diagnostics:done:{}", job.id))?
                .unwrap_or(false)
            {
                continue;
            }
            if !self.adapters.contains_key(&job.plugin)
                || !safe_component(&job.version)
                || !(1..=3600).contains(&job.timeout_secs)
                || job
                    .resource_budget
                    .as_ref()
                    .is_some_and(|budget| !budget.valid())
                || expired(&job)
            {
                self.finish(failure(
                    job.id,
                    "unsupported diagnostic plugin, invalid job, or expired task".into(),
                    None,
                ))?;
                continue;
            }
            self.save(&Checkpoint::Preparing(job))?;
            break;
        }
        Ok(())
    }

    async fn prepare(&self, job: DiagnosticJob, client: &PanelClient) -> Result<()> {
        let id = job.id;
        let result: Result<()> = async {
            ensure!(!self.cancellation_requested(id)?, "diagnostic cancellation was requested");
            ensure!(!expired(&job), "diagnostic task has expired");
            let adapter = self.adapters.get(&job.plugin).context("diagnostic plugin is not registered")?;
            let descriptor = adapter.describe();
            let auxiliary_files = adapter.auxiliary_files_for_version(&job.version);
            let signed_descriptor = Descriptor {
                auxiliary_files: auxiliary_files.clone(),
                module: "diagnostics".into(), plugin_name: descriptor.plugin_name.clone(),
                binary_name: descriptor.binary_name.clone(), service_unit: String::new(), service_group: String::new(),
            };
            let binary = tokio::time::timeout(Duration::from_secs(300), client.ensure_artifact(&job.artifact, &job.version, &Descriptor {
                module: "diagnostics".into(), plugin_name: descriptor.plugin_name,
                binary_name: descriptor.binary_name, auxiliary_files, service_unit: String::new(), service_group: String::new(),
            }, &self.config.install_root, self.privileged.as_ref())).await.context("diagnostic artifact installation timed out")??;
            ensure!(!expired(&job), "diagnostic task expired during artifact installation");
            let keys = self.keys.as_ref().map_err(|error| anyhow::anyhow!(error.to_string()))?;
            crate::artifacts::verify_expected(&binary, &signed_descriptor, &job.version, keys).await?;
            let mut spec = DiagnosticSpec {
                id: id.to_string(), version: job.version,
                binary_path: binary,
                job_dir: self.config.runtime_root.join("diagnostics").join(id.to_string()),
                timeout_secs: execution_timeout(job.timeout_secs, job.expires_at, unix_time())?, options: job.options,
            };
            let mut service = self.bounded(adapter.prepare(&spec, self.privileged.as_ref())).await?;
            ensure!(service.unit == format!("apsp-diagnostic-{id}.service")
                && service.timeout_secs == spec.timeout_secs
                && service.working_directory == spec.job_dir && service.program == spec.binary_path, "invalid prepared diagnostic service");
            if let Some(budget)=&job.resource_budget { budget::apply(budget,&mut service)?; }
            let resources = self.preflight(&service).await?;
            let started_at = unix_time();
            // Preparation can consume part of the remaining budget. Recompute immediately
            // before the durable start checkpoint and give systemd the reduced limit.
            let timeout_secs = execution_timeout(job.timeout_secs, job.expires_at, started_at)?;
            spec.timeout_secs = timeout_secs;
            service.timeout_secs = timeout_secs;
            crate::artifacts::verify_expected(&spec.binary_path, &signed_descriptor, &spec.version, keys).await?;
            // This checkpoint is durable before asking systemd to start anything. Recovery
            // only observes it; an uncertain start must never execute the task again.
            let mut checkpoint = Checkpoint::Started {
                environment: Some(environment::ExecutionEnvironment::capture(&service, &resources, started_at)),
                spec, service: service.clone(), started_at, plugin: job.plugin, start_error: None, expires_at: job.expires_at, protection_stop_reason: None, terminal_update: None, cleanup_error: None,
            };
            ensure!(!self.cancellation_requested(id)?, "diagnostic cancellation was requested before service start");
            self.save(&checkpoint)?;
            self.capture_environment(&checkpoint)?;
            if let Err(error) = self.bounded(self.services.start_job(&service)).await {
                tracing::warn!(%id, %error, "diagnostic start response is uncertain; inspecting service on next poll");
                if let Checkpoint::Started { start_error, .. } = &mut checkpoint {
                    *start_error = Some(error.to_string().chars().take(4096).collect());
                }
                self.save_if_owned(&checkpoint)?;
            }
            Ok(())
        }.await;
        if let Err(error) = result
            && !self.cancellation_requested(id)?
        {
            match self.active()? {
                Some(Checkpoint::Started { .. }) => {
                    // A durable start checkpoint owns a possibly running unit, even
                    // when recording the environment or uncertain response fails.
                    tracing::warn!(%id, %error, "diagnostic preparation left a durable start; observation retains cleanup ownership");
                }
                Some(Checkpoint::Preparing(known)) if known.id == id => {
                    self.finish(failure(id, format!("诊断准备失败：{error:#}"), None))?;
                }
                _ => return Err(error),
            }
        }
        Ok(())
    }

    pub async fn tick(&self, client: Option<&PanelClient>) -> Result<()> {
        self.process_cancellations().await?;
        if let Some(checkpoint @ Checkpoint::Started { .. }) = self.active()? {
            self.observe_and_notify(&checkpoint, client).await?;
        }
        self.connected_tick(client).await
    }

    async fn connected_tick(&self, client: Option<&PanelClient>) -> Result<()> {
        let Some(client) = client else {
            return Ok(());
        };
        // Pending terminal results are retried before another task is accepted.
        self.flush(client).await?;
        if self.active()?.is_none() {
            self.accept(self.bounded(client.diagnostic_jobs()).await?)?;
        }
        match self.active()? {
            Some(Checkpoint::Preparing(job)) => self.prepare(job, client).await?,
            Some(checkpoint @ Checkpoint::Started { .. }) => {
                self.bounded(client.diagnostic_update(&self.current_status_update(&checkpoint)?))
                    .await?
            }
            None => {}
        }
        Ok(())
    }
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn expired(job: &DiagnosticJob) -> bool {
    job.expires_at
        .is_some_and(|deadline| deadline <= unix_time() as i64)
}

fn execution_timeout(configured: u64, expires_at: Option<i64>, now: u64) -> Result<u32> {
    let remaining = match expires_at {
        Some(deadline) => u64::try_from(deadline)
            .ok()
            .and_then(|deadline| deadline.checked_sub(now))
            .filter(|remaining| *remaining > 0)
            .context("diagnostic task expired before service start")?,
        None => configured,
    };
    ensure!(
        (1..=3600).contains(&configured),
        "invalid diagnostic timeout"
    );
    Ok(u32::try_from(configured.min(remaining))?)
}

fn failure(id: Uuid, error: String, report: Option<DiagnosticReport>) -> DiagnosticUpdate {
    DiagnosticUpdate {
        id,
        status: DiagnosticStatus::Failed,
        report,
        error: Some(error.chars().take(4096).collect()),
    }
}

#[cfg(test)]
#[path = "diagnostics/tests.rs"]
mod tests;

pub(crate) async fn stop_for_retirement(
    config: &Config,
    state: &SharedState,
    services: &dyn ServiceManager,
    timeout_secs: u64,
) -> Result<()> {
    let active: Option<Checkpoint> = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
        .get_json::<Option<Checkpoint>>(ACTIVE)?
        .flatten();
    if let Some(Checkpoint::Started { spec, service, .. }) = active {
        finalization::validate_saved_target(config, &spec, &service)?;
        tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            finalization::stop_and_confirm(services, &service.unit, &spec.job_dir),
        )
        .await
        .context("diagnostic retirement timed out")??;
    }
    Ok(())
}

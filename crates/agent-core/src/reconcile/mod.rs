mod apply;
mod checkpoint;
mod constraints;
mod inspection;
mod operations;
mod path_probe;

use crate::{artifacts::PanelClient, config::Config, state::SharedState};
use anyhow::{Context, Result};
use apsp_adapter_sdk::{Adapter, Plan, Prepared, Privileged, RuntimeSpec, ServiceManager};
use apsp_protocol::release::{ReleaseError, TrustedKeys};
use apsp_protocol::{ApplyResult, ApplyStatus, ModuleManifest};
use serde::{Deserialize, Serialize};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::timeout};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApplyIntent {
    pub previous: Option<Prepared>,
    pub target: Prepared,
    pub plan: Plan,
}

pub struct Reconciler {
    config: Config,
    state: SharedState,
    adapter: Arc<dyn Adapter>,
    privileged: Arc<dyn Privileged>,
    services: Arc<dyn ServiceManager>,
    gate: Mutex<()>,
    pending_manifest: std::sync::Mutex<Option<ModuleManifest>>,
    pending_prepared: std::sync::Mutex<Option<Prepared>>,
    keys: std::result::Result<TrustedKeys, ReleaseError>,
}

#[derive(Debug)]
pub struct RecoveryBlocked(pub(crate) String);
impl std::fmt::Display for RecoveryBlocked {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for RecoveryBlocked {}

impl Reconciler {
    pub fn supports_runtime_probe(&self) -> bool {
        self.services.supports_runtime_checkpoint() && self.adapter.supports_runtime_probe()
    }

    pub fn new(
        config: Config,
        state: SharedState,
        adapter: Arc<dyn Adapter>,
        privileged: Arc<dyn Privileged>,
        services: Arc<dyn ServiceManager>,
    ) -> Self {
        Self {
            config,
            state,
            adapter,
            privileged,
            services,
            gate: Mutex::new(()),
            pending_manifest: std::sync::Mutex::new(None),
            pending_prepared: std::sync::Mutex::new(None),
            keys: TrustedKeys::compiled(),
        }
    }

    /// Supplies roots already trusted by an embedding caller, including test fixtures.
    pub fn with_trusted_keys(mut self, keys: TrustedKeys) -> Self {
        self.keys = Ok(keys);
        self
    }

    pub(crate) fn management_recovery_allowed(&self, error: &anyhow::Error) -> bool {
        self.keys.is_ok() && error.downcast_ref::<RecoveryBlocked>().is_some()
    }

    async fn verify_runtime(&self, runtime: &Prepared) -> Result<()> {
        let descriptor = self.adapter.describe();
        let result: Result<()> = async {
            self.enforce_generation_floor(runtime)?;
            anyhow::ensure!(
                runtime.spec.binary_path
                    == self
                        .config
                        .install_root
                        .join(&descriptor.plugin_name)
                        .join(&runtime.spec.kernel_version)
                        .join(&descriptor.binary_name),
                "runtime binary path differs from installation identity"
            );
            let keys = self
                .keys
                .as_ref()
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            crate::artifacts::verify_expected(
                &runtime.spec.binary_path,
                &descriptor,
                &runtime.spec.kernel_version,
                keys,
            )
            .await
        }
        .await;
        if let Err(error) = &result {
            self.state.lock().map_err(|_| anyhow::anyhow!("state poisoned"))?
                .set_json(&format!("artifact_error:{}", descriptor.module), &serde_json::json!({"revision":runtime.spec.revision,"error":error.to_string()}))?;
            tracing::error!(module=%descriptor.module, revision=runtime.spec.revision, %error, "artifact verification failed; executable was not started");
        }
        result
    }

    async fn verify_applied_runtime(&self, runtime: &Prepared) -> Result<()> {
        let result = self.verify_runtime(runtime).await;
        if result.is_err() {
            self.state
                .lock()
                .map_err(|_| anyhow::anyhow!("state poisoned"))?
                .set_json(
                    &format!("health:{}", self.adapter.describe().module),
                    &false,
                )?;
        }
        result
    }

    pub async fn recover(&self) -> Result<()> {
        let _guard = self.gate.lock().await;
        self.recover_locked().await
    }

    async fn recover_locked(&self) -> Result<()> {
        let module = self.adapter.describe().module;
        let pending = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?
            .pending_intents()?;
        for record in pending.into_iter().filter(|record| record.module == module) {
            let intent: ApplyIntent = serde_json::from_value(record.payload)?;
            tracing::warn!(%module, op_id = %record.op_id, "recovering unfinished operation");
            if let Err(error) = self.rollback(&intent, record.op_id).await {
                self.state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state poisoned"))?
                    .set_json(&format!("health:{module}"), &false)?;
                if error.downcast_ref::<RecoveryBlocked>().is_some() {
                    self.state.lock().map_err(|_| anyhow::anyhow!("state poisoned"))?.set_json(
                        &format!("runtime_recovery_error:{module}"),
                        &serde_json::json!({"revision": intent.target.spec.revision, "error": error.to_string()}),
                    )?;
                    return Err(error);
                }
                return Err(error.context(
                    "unfinished operation could not recover a trusted artifact; intent retained",
                ));
            }
        }
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?
            .remove_json(&format!("runtime_recovery_error:{module}"))?;
        Ok(())
    }

    fn previous(&self) -> Result<Option<Prepared>> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?
            .get_json(&format!("applied:{}", self.adapter.describe().module))
    }

    async fn bounded<T>(&self, future: impl Future<Output = Result<T>>) -> Result<T> {
        timeout(
            Duration::from_secs(self.config.operation_timeout_secs),
            future,
        )
        .await
        .context("operation timed out")?
    }

    async fn runtime_health(&self, runtime: &Prepared) -> Result<bool> {
        let budget = Duration::from_secs(self.config.operation_timeout_secs).max(
            self.adapter
                .health_timeout(runtime)
                .min(Duration::from_secs(300)),
        );
        timeout(budget, self.adapter.health(runtime, self.services.as_ref()))
            .await
            .context("runtime health check timed out")?
    }

    pub async fn reconcile(
        &self,
        manifest: &ModuleManifest,
        client: &PanelClient,
    ) -> Result<ApplyResult> {
        {
            let mut pending = self
                .pending_manifest
                .lock()
                .map_err(|_| anyhow::anyhow!("pending target poisoned"))?;
            if pending
                .as_ref()
                .is_none_or(|current| manifest.config_rev >= current.config_rev)
            {
                *pending = Some(manifest.clone());
            }
        }
        let _guard = self.gate.lock().await;
        let next = self
            .pending_manifest
            .lock()
            .map_err(|_| anyhow::anyhow!("pending target poisoned"))?
            .take();
        let Some(manifest) = next else {
            return self.current_result().await;
        };
        let op_id = Uuid::new_v4();
        let module = self.adapter.describe().module;
        let result = self.reconcile_locked(&manifest, client, op_id).await;
        match result {
            Ok(rev) => self.stable_apply_result(ApplyResult {
                module,
                rev,
                op_id,
                status: ApplyStatus::Applied,
                healthy: true,
                error: None,
            }),
            Err(error) => {
                let healthy = self
                    .state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state poisoned"))?
                    .get_json::<bool>(&format!("health:{module}"))?
                    .unwrap_or(false);
                tracing::warn!(%module,rev=manifest.config_rev,error=%error,"application failed");
                self.stable_apply_result(ApplyResult {
                    module,
                    rev: manifest.config_rev,
                    op_id,
                    status: ApplyStatus::Failed,
                    healthy,
                    error: Some(format!("{error:#}")),
                })
            }
        }
    }

    async fn reconcile_locked(
        &self,
        manifest: &ModuleManifest,
        client: &PanelClient,
        op_id: Uuid,
    ) -> Result<u64> {
        self.recover_locked().await?;
        let descriptor = self.adapter.describe();
        self.require_revision_floor(manifest.config_rev)?;
        client.verify_artifact(&manifest.artifact, &manifest.kernel_version, &descriptor)?;
        let previous = self.previous()?;
        if let Some(previous) = &previous {
            self.verify_applied_runtime(previous).await?;
            if manifest.config_rev < previous.spec.revision {
                anyhow::ensure!(
                    self.runtime_health(previous).await?,
                    "last applied runtime is unhealthy"
                );
                return Ok(previous.spec.revision);
            }
            if manifest.config_rev == previous.spec.revision {
                anyhow::ensure!(
                    manifest.bundle_sha256 == previous.spec.config_hash,
                    "same revision changed content"
                );
            }
            if manifest.config_rev == previous.spec.revision
                && manifest.kernel_version == previous.spec.kernel_version
            {
                let healthy = self.runtime_health(previous).await?;
                self.state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state poisoned"))?
                    .set_json(&format!("health:{}", descriptor.module), &healthy)?;
                anyhow::ensure!(healthy, "last applied runtime is unhealthy");
                if self.services.supports_runtime_checkpoint()
                    && let Some(activation) = self.saved_activation()?
                {
                    self.validate_activation(previous, &activation).await?;
                }
                return Ok(previous.spec.revision);
            }
        }
        let binary_path = timeout(
            Duration::from_secs(300),
            client.ensure_artifact(
                &manifest.artifact,
                &manifest.kernel_version,
                &descriptor,
                &self.config.install_root,
                self.privileged.as_ref(),
            ),
        )
        .await
        .context("artifact installation timed out")??;
        let bundle = client
            .bundle(&manifest.bundle_url, &manifest.bundle_sha256)
            .await?;
        let runtime_root = self
            .config
            .runtime_root
            .join(format!("{}@main", descriptor.plugin_name));
        let revision_dir = runtime_root
            .join("revisions")
            .join(manifest.config_rev.to_string());
        let group =
            (!descriptor.service_group.is_empty()).then_some(descriptor.service_group.as_str());
        self.bounded(self.privileged.create_dir(&runtime_root, 0o750, group))
            .await?;
        self.bounded(
            self.privileged
                .create_dir(&runtime_root.join("revisions"), 0o750, group),
        )
        .await?;
        self.bounded(self.privileged.create_dir(&revision_dir, 0o750, group))
            .await?;
        for (name, contents) in &bundle.files {
            anyhow::ensure!(
                std::path::Path::new(name)
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
                    && !name.is_empty(),
                "unsafe bundle path"
            );
            let path = revision_dir.join(name);
            if let Some(parent) = path.parent() {
                self.bounded(self.privileged.create_dir(parent, 0o750, group))
                    .await?;
            }
            self.bounded(
                self.privileged
                    .write_file(&path, contents.as_bytes(), 0o640, group),
            )
            .await?;
        }
        let spec = RuntimeSpec {
            revision: manifest.config_rev,
            kernel_version: manifest.kernel_version.clone(),
            config_hash: manifest.bundle_sha256.clone(),
            binary_path,
            revision_dir,
            stats_listen: manifest.stats_listen.clone(),
            files: bundle.files,
        };
        let candidate = Prepared {
            spec,
            listen_ports: Vec::new(),
        };
        self.verify_runtime(&candidate).await?;
        let target = self
            .bounded(
                self.adapter
                    .prepare(candidate.spec, self.privileged.as_ref()),
            )
            .await?;
        self.apply_prepared_locked(previous, target, op_id).await?;
        Ok(manifest.config_rev)
    }
}

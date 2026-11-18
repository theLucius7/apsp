use crate::state::Locked;
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
    bundled: Option<crate::bundled::BundledItem>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartupPlan {
    Idle,
    SyncDesired,
    AwaitPanelIntent,
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
    /// Session observation for concurrent client address limits, if supported.
    pub fn session_source(&self) -> Option<&dyn apsp_adapter_sdk::SessionSource> {
        self.adapter.session_source()
    }

    /// The runtime currently applied, without taking the apply gate.
    pub fn applied_runtime(&self) -> Result<Option<Prepared>> {
        self.previous()
    }

    pub fn descriptor(&self) -> apsp_adapter_sdk::Descriptor {
        self.adapter.describe()
    }

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
            bundled: None,
        }
    }

    /// Supplies roots already trusted by an embedding caller, including test fixtures.
    pub fn with_trusted_keys(mut self, keys: TrustedKeys) -> Self {
        self.keys = Ok(keys);
        self
    }

    /// Supplies a compiled-in runtime component materialized on demand when configured.
    pub fn with_bundled_runtime(mut self, item: crate::bundled::BundledItem) -> Self {
        self.bundled = Some(item);
        self
    }

    pub(crate) fn bundled_runtime(&self) -> Option<&crate::bundled::BundledItem> {
        self.bundled.as_ref()
    }

    pub(super) async fn ensure_service_definition(&self) -> Result<()> {
        let spec = apsp_adapter_sdk::RuntimeServiceSpec {
            descriptor: self.adapter.describe(),
            agent_root: self.config.agent_root.clone(),
            install_root: self.config.install_root.clone(),
            runtime_root: self.config.runtime_root.clone(),
        };
        self.bounded(self.services.ensure_runtime_service(&spec))
            .await
    }

    pub(super) async fn local_runtime_links_and_files_match(
        &self,
        runtime: &Prepared,
    ) -> Result<bool> {
        let descriptor = self.adapter.describe();
        let install_link = self
            .config
            .install_root
            .join(&descriptor.artifact_name)
            .join("current");
        let Ok(install_target) = crate::system::deploy::reference(&install_link) else {
            return Ok(false);
        };
        let Ok(install_dir) = tokio::fs::canonicalize(&install_target).await else {
            return Ok(false);
        };
        let Ok(expected_install_dir) = tokio::fs::canonicalize(
            runtime
                .spec
                .binary_path
                .parent()
                .context("runtime binary has no parent")?,
        )
        .await
        else {
            return Ok(false);
        };
        if install_dir != expected_install_dir {
            return Ok(false);
        }

        let runtime_link = self
            .config
            .runtime_root
            .join(format!("{}@main", descriptor.artifact_name))
            .join("current");
        let Ok(runtime_target) = crate::system::deploy::reference(&runtime_link) else {
            return Ok(false);
        };
        let Ok(runtime_dir) = tokio::fs::canonicalize(&runtime_target).await else {
            return Ok(false);
        };
        let Ok(expected_runtime_dir) = tokio::fs::canonicalize(&runtime.spec.revision_dir).await
        else {
            return Ok(false);
        };
        if runtime_dir != expected_runtime_dir {
            return Ok(false);
        }

        for (name, expected_contents) in &runtime.spec.files {
            let file_path = runtime.spec.revision_dir.join(name);
            let Ok(meta) = tokio::fs::symlink_metadata(&file_path).await else {
                return Ok(false);
            };
            if !meta.is_file()
                || meta.file_type().is_symlink()
                || meta.len() != expected_contents.len() as u64
            {
                return Ok(false);
            }
            let Ok(actual) = tokio::fs::read(&file_path).await else {
                return Ok(false);
            };
            if actual != expected_contents.as_bytes() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) async fn active_component_matches_bundled(
        &self,
        runtime: &Prepared,
        activation: Option<&checkpoint::Activation>,
    ) -> Result<bool> {
        if !self.local_runtime_links_and_files_match(runtime).await? {
            return Ok(false);
        }
        let Some(item) = &self.bundled else {
            return Ok(true);
        };
        if runtime.spec.kernel_version != item.version {
            return Ok(false);
        }
        let Some(manifest) =
            crate::bundled::current_matches_compiled(item, &self.config.install_root).await?
        else {
            return Ok(false);
        };
        if self.services.supports_runtime_checkpoint()
            && activation.and_then(|act| act.binary_sha256.as_deref())
                != Some(manifest.binary_sha256.as_str())
        {
            return Ok(false);
        }
        Ok(true)
    }

    pub(crate) fn management_recovery_allowed(&self, error: &anyhow::Error) -> bool {
        (self.keys.is_ok() || self.bundled.is_some())
            && error.downcast_ref::<RecoveryBlocked>().is_some()
    }

    async fn verify_runtime(&self, runtime: &Prepared) -> Result<()> {
        let descriptor = self.adapter.describe();
        let result: Result<()> = async {
            self.enforce_generation_floor(runtime)?;
            let version_dir = runtime
                .spec
                .binary_path
                .parent()
                .context("runtime binary has no parent")?;
            let artifact_dir = version_dir
                .parent()
                .context("runtime binary has no artifact directory")?;
            let dir_name = version_dir
                .file_name()
                .and_then(|name| name.to_str())
                .context("invalid runtime version directory")?;
            anyhow::ensure!(
                artifact_dir == self.config.install_root.join(&descriptor.artifact_name)
                    && crate::bundled::matches_version_directory_name(
                        dir_name,
                        &runtime.spec.kernel_version,
                    )
                    && runtime
                        .spec
                        .binary_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        == Some(descriptor.binary_name.as_str()),
                "runtime binary path differs from installation identity"
            );
            if let Some(manifest) = crate::bundled::read_bundled_manifest(version_dir).await? {
                anyhow::ensure!(
                    manifest.component_name == descriptor.artifact_name
                        && manifest.binary_name == descriptor.binary_name
                        && manifest.version == runtime.spec.kernel_version,
                    "installed bundled runtime identity mismatch"
                );
                return crate::bundled::verify_manifest_files(version_dir, &manifest).await;
            }
            let keys = self
                .keys
                .as_ref()
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            let descriptor = crate::artifacts::installed_descriptor(
                self.adapter.as_ref(),
                &runtime.spec.kernel_version,
                &runtime.spec.binary_path,
                keys,
            )
            .await?;
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
            self.state.locked()?.set_json(
                &format!("artifact_error:{}", descriptor.module),
                &serde_json::json!({"revision":runtime.spec.revision,"error":error.to_string()}),
            )?;
            tracing::error!(module=%descriptor.module, revision=runtime.spec.revision, %error, "artifact verification failed; executable was not started");
        }
        result
    }

    async fn verify_applied_runtime(&self, runtime: &Prepared) -> Result<()> {
        let result = self.verify_runtime(runtime).await;
        if result.is_err() {
            self.state.locked()?.set_json(
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
        let pending = self.state.locked()?.pending_intents()?;
        for record in pending.into_iter().filter(|record| record.module == module) {
            let intent: ApplyIntent = serde_json::from_value(record.payload)?;
            tracing::warn!(%module, op_id = %record.op_id, "recovering unfinished operation");
            if let Err(error) = self.rollback(&intent, record.op_id).await {
                self.state
                    .locked()?
                    .set_json(&format!("health:{module}"), &false)?;
                if error.downcast_ref::<RecoveryBlocked>().is_some() {
                    self.state.locked()?.set_json(
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
            .locked()?
            .remove_json(&format!("runtime_recovery_error:{module}"))?;
        Ok(())
    }

    fn previous(&self) -> Result<Option<Prepared>> {
        self.state
            .locked()?
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
                    .locked()?
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

    /// Stops and clears an active runtime when the panel's desired manifest omits this module,
    /// without touching unconfigured monitoring hosts that have no active runtime.
    pub async fn deactivate_if_active(&self) -> Result<bool> {
        let _guard = self.gate.lock().await;
        self.recover_locked().await?;
        let descriptor = self.adapter.describe();
        let desired_key = format!("desired:{}", descriptor.module);
        let desired = self.state.locked()?.get_json::<bool>(&desired_key)?;
        let previous = self.previous()?;
        if previous.is_none() {
            if desired == Some(false) {
                return Ok(false);
            }
            let mut state = self.state.locked()?;
            state.remove_json(&format!("apply_receipt:{}", descriptor.module))?;
            state.set_json(&desired_key, &false)?;
            state.set_json(
                &format!("runtime_activation:{}", descriptor.module),
                &Option::<checkpoint::Activation>::None,
            )?;
            state.set_json(&format!("health:{}", descriptor.module), &false)?;
            return Ok(false);
        }
        // Persist explicit panel deactivation before stopping so an interrupted stop never revives.
        self.state.locked()?.set_json(&desired_key, &false)?;
        if let Some(previous) = &previous {
            if let Err(error) = self.sample_runtime(previous).await {
                tracing::warn!(
                    module = %descriptor.module,
                    %error,
                    "cannot capture terminal counters while stopping unconfigured runtime"
                );
            }
            if self
                .bounded(self.services.is_active(&descriptor.service_unit))
                .await
                .unwrap_or(true)
            {
                self.bounded(self.services.stop(&descriptor.service_unit))
                    .await?;
            }
            let runtime_link = self
                .config
                .runtime_root
                .join(format!("{}@main", descriptor.artifact_name))
                .join("current");
            let _ = self
                .bounded(self.privileged.remove_symlink(&runtime_link))
                .await;
        }
        {
            let mut state = self.state.locked()?;
            state.remove_json(&format!("applied:{}", descriptor.module))?;
            state.remove_json(&format!("apply_receipt:{}", descriptor.module))?;
            state.set_json(&desired_key, &false)?;
            state.set_json(
                &format!("runtime_activation:{}", descriptor.module),
                &Option::<checkpoint::Activation>::None,
            )?;
            state.set_json(&format!("health:{}", descriptor.module), &false)?;
        }
        Ok(true)
    }

    /// Evaluates startup runtime disposition across the three explicit `desired` states:
    /// - `Some(false)`: explicitly disabled by panel; finishes any interrupted stop and stays idle.
    /// - `Some(true)`: confirmed active desire; schedules built-in runtime sync before trial pass.
    /// - `None`: legacy database without explicit panel intent; preserves an already verified
    ///   live service without waking stopped history, awaiting authenticated panel manifest.
    pub(crate) async fn startup_readiness_plan(&self) -> Result<StartupPlan> {
        let descriptor = self.adapter.describe();
        let desired_key = format!("desired:{}", descriptor.module);
        let desired = self.state.locked()?.get_json::<bool>(&desired_key)?;
        let previous = self.previous()?;
        match desired {
            Some(false) => {
                if previous.is_some() {
                    let _ = self.deactivate_if_active().await?;
                }
                Ok(StartupPlan::Idle)
            }
            Some(true) => {
                if previous.is_some() && self.bundled.is_some() {
                    Ok(StartupPlan::SyncDesired)
                } else {
                    Ok(StartupPlan::Idle)
                }
            }
            None => {
                let Some(previous) = previous else {
                    return Ok(StartupPlan::Idle);
                };
                let _guard = self.gate.lock().await;
                let verified_live = self.require_revision_floor(previous.spec.revision).is_ok()
                    && self.verify_applied_runtime(&previous).await.is_ok()
                    && self
                        .bounded(self.services.is_active(&descriptor.service_unit))
                        .await
                        .unwrap_or(false)
                    && self.runtime_health(&previous).await.unwrap_or(false)
                    && (!self.services.supports_runtime_checkpoint()
                        || match self.saved_activation()? {
                            Some(act) => self
                                .validate_legacy_or_current_activation(&previous, &act)
                                .await
                                .is_ok(),
                            None => true,
                        });
                self.state
                    .locked()?
                    .set_json(&format!("health:{}", descriptor.module), &verified_live)?;
                if self.bundled.is_some() {
                    Ok(StartupPlan::AwaitPanelIntent)
                } else {
                    Ok(StartupPlan::Idle)
                }
            }
        }
    }

    async fn validate_legacy_or_current_activation(
        &self,
        runtime: &Prepared,
        activation: &checkpoint::Activation,
    ) -> Result<()> {
        if activation.binary_sha256.is_some() {
            return self.validate_activation(runtime, activation).await;
        }
        anyhow::ensure!(
            self.inspect_runtime(runtime).await? == activation.instance_snapshot()
                && activation.bundle_sha256 == runtime.spec.config_hash,
            "legacy activation differs from running instance"
        );
        Ok(())
    }

    pub(crate) async fn satisfies_trial_readiness(&self) -> Result<bool> {
        let _guard = self.gate.lock().await;
        let descriptor = self.adapter.describe();
        let desired_key = format!("desired:{}", descriptor.module);
        let desired = self.state.locked()?.get_json::<bool>(&desired_key)?;
        let previous = self.previous()?;
        match desired {
            Some(false) => Ok(previous.is_none()),
            None => Ok(previous.is_none() || self.bundled.is_none()),
            Some(true) => {
                let Some(previous) = previous else {
                    return Ok(false);
                };
                if self.require_revision_floor(previous.spec.revision).is_err() {
                    return Ok(false);
                }
                if self.verify_applied_runtime(&previous).await.is_err() {
                    return Ok(false);
                }
                let is_active = self
                    .bounded(self.services.is_active(&descriptor.service_unit))
                    .await
                    .unwrap_or(false);
                let recorded_healthy = self
                    .state
                    .locked()?
                    .get_json::<bool>(&format!("health:{}", descriptor.module))?
                    .unwrap_or(false);
                if !is_active || !recorded_healthy {
                    return Ok(false);
                }
                let activation = if self.services.supports_runtime_checkpoint() {
                    self.saved_activation()?
                } else {
                    None
                };
                if self.services.supports_runtime_checkpoint() {
                    let Some(act) = &activation else {
                        return Ok(false);
                    };
                    if self.validate_activation(&previous, act).await.is_err() {
                        return Ok(false);
                    }
                }
                self.active_component_matches_bundled(&previous, activation.as_ref())
                    .await
            }
        }
    }

    /// Ensures any explicitly desired applied runtime (`desired == Some(true)`) is running the
    /// Agent's built-in runtime version and SHA-256 and passes health check before trial ready.
    /// Never wakes historical records whose panel intent is disabled or unknown.
    pub async fn ensure_startup_desired_runtime(&self) -> Result<()> {
        let _guard = self.gate.lock().await;
        let descriptor = self.adapter.describe();
        let desired_key = format!("desired:{}", descriptor.module);
        let desired = self.state.locked()?.get_json::<bool>(&desired_key)?;
        if desired != Some(true) {
            return Ok(());
        }
        let Some(previous) = self.previous()? else {
            return Ok(());
        };
        self.require_revision_floor(previous.spec.revision)?;
        let Some(bundled_item) = &self.bundled else {
            return Ok(());
        };
        let verified_prior = self.verify_applied_runtime(&previous).await.is_ok();
        let is_active = self
            .bounded(self.services.is_active(&descriptor.service_unit))
            .await
            .unwrap_or(false);
        let healthy =
            verified_prior && is_active && self.runtime_health(&previous).await.unwrap_or(false);
        let activation = if self.services.supports_runtime_checkpoint() {
            self.saved_activation()?
        } else {
            None
        };
        let activation_ok = if self.services.supports_runtime_checkpoint() {
            match &activation {
                Some(act) => self.validate_activation(&previous, act).await.is_ok(),
                None => false,
            }
        } else {
            true
        };
        let component_ok = self
            .active_component_matches_bundled(&previous, activation.as_ref())
            .await
            .unwrap_or(false);
        if healthy && activation_ok && component_ok {
            self.state
                .locked()?
                .set_json(&format!("health:{}", descriptor.module), &true)?;
            return Ok(());
        }
        if !healthy {
            self.state
                .locked()?
                .set_json(&format!("health:{}", descriptor.module), &false)?;
        }

        let binary_path = crate::bundled::ensure_materialized(
            bundled_item,
            &self.config.install_root,
            self.privileged.as_ref(),
        )
        .await?;
        let group =
            (!descriptor.service_group.is_empty()).then_some(descriptor.service_group.as_str());
        self.stage_revision_files(&previous.spec.revision_dir, &previous.spec.files, group)
            .await?;
        let candidate_spec = RuntimeSpec {
            revision: previous.spec.revision,
            kernel_version: bundled_item.version.clone(),
            config_hash: previous.spec.config_hash.clone(),
            binary_path,
            revision_dir: previous.spec.revision_dir.clone(),
            stats_listen: previous.spec.stats_listen.clone(),
            files: previous.spec.files.clone(),
        };
        self.verify_runtime(&Prepared {
            spec: candidate_spec.clone(),
            listen_ports: Vec::new(),
        })
        .await?;
        let target = self
            .bounded(
                self.adapter
                    .prepare(candidate_spec, self.privileged.as_ref()),
            )
            .await?;
        self.apply_plan_locked(Some(previous), target, Plan::Restart, Uuid::new_v4())
            .await
    }

    async fn stage_revision_files(
        &self,
        revision_dir: &std::path::Path,
        files: &std::collections::BTreeMap<String, String>,
        group: Option<&str>,
    ) -> Result<()> {
        let descriptor = self.adapter.describe();
        let runtime_root = self
            .config
            .runtime_root
            .join(format!("{}@main", descriptor.artifact_name));
        self.bounded(self.privileged.create_dir(&runtime_root, 0o750, group))
            .await?;
        self.bounded(
            self.privileged
                .create_dir(&runtime_root.join("revisions"), 0o750, group),
        )
        .await?;
        self.bounded(self.privileged.create_dir(revision_dir, 0o750, group))
            .await?;
        for (name, contents) in files {
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
        Ok(())
    }

    async fn reconcile_locked(
        &self,
        manifest: &ModuleManifest,
        client: &PanelClient,
        op_id: Uuid,
    ) -> Result<u64> {
        self.recover_locked().await?;
        self.require_revision_floor(manifest.config_rev)?;
        let base_descriptor = self.adapter.describe();
        self.state
            .locked()?
            .set_json(&format!("desired:{}", base_descriptor.module), &true)?;

        let (descriptor, effective_kernel_version) = if let Some(bundled_item) = &self.bundled {
            anyhow::ensure!(
                manifest.kernel_version.is_empty()
                    || manifest.kernel_version == bundled_item.version,
                "requested runtime version {} is incompatible with built-in version {}",
                manifest.kernel_version,
                bundled_item.version
            );
            (base_descriptor, bundled_item.version.clone())
        } else if let Some(artifact) = &manifest.artifact {
            anyhow::ensure!(
                !manifest.kernel_version.is_empty(),
                "runtime manifest has no kernel version"
            );
            let keys = client.keys()?;
            let binary = self
                .config
                .install_root
                .join(&base_descriptor.artifact_name)
                .join(&manifest.kernel_version)
                .join(&base_descriptor.binary_name);
            let descriptor = crate::artifacts::installed_descriptor(
                self.adapter.as_ref(),
                &manifest.kernel_version,
                &binary,
                keys,
            )
            .await?;
            let _ = crate::artifacts::verify_local_cached_artifact(
                artifact,
                &manifest.kernel_version,
                &descriptor,
                &self.config.install_root,
                keys,
            )
            .await?;
            (descriptor, manifest.kernel_version.clone())
        } else {
            anyhow::bail!(
                "runtime manifest has no artifact and Agent has no built-in runtime for module {}",
                base_descriptor.module
            );
        };

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
                let is_active = self
                    .bounded(self.services.is_active(&descriptor.service_unit))
                    .await
                    .unwrap_or(false);
                let healthy = is_active && self.runtime_health(previous).await.unwrap_or(false);
                let activation = if self.services.supports_runtime_checkpoint() {
                    self.saved_activation()?
                } else {
                    None
                };
                let activation_ok = if self.services.supports_runtime_checkpoint() {
                    match &activation {
                        Some(act) => self.validate_activation(previous, act).await.is_ok(),
                        None => false,
                    }
                } else {
                    true
                };
                let component_ok = previous.spec.kernel_version == effective_kernel_version
                    && self
                        .active_component_matches_bundled(previous, activation.as_ref())
                        .await
                        .unwrap_or(false);
                if healthy && activation_ok && component_ok {
                    self.state
                        .locked()?
                        .set_json(&format!("health:{}", descriptor.module), &true)?;
                    return Ok(previous.spec.revision);
                }
                if !healthy {
                    self.state
                        .locked()?
                        .set_json(&format!("health:{}", descriptor.module), &false)?;
                }
            }
        }

        let binary_path = if let Some(bundled_item) = &self.bundled {
            crate::bundled::ensure_materialized(
                bundled_item,
                &self.config.install_root,
                self.privileged.as_ref(),
            )
            .await?
        } else {
            let artifact = manifest
                .artifact
                .as_ref()
                .context("runtime manifest has no artifact")?;
            crate::artifacts::verify_local_cached_artifact(
                artifact,
                &effective_kernel_version,
                &descriptor,
                &self.config.install_root,
                client.keys()?,
            )
            .await?
        };
        let bundle = client
            .bundle(&manifest.bundle_url, &manifest.bundle_sha256)
            .await?;
        let runtime_root = self
            .config
            .runtime_root
            .join(format!("{}@main", descriptor.artifact_name));
        let revision_dir = runtime_root
            .join("revisions")
            .join(manifest.config_rev.to_string());
        let group =
            (!descriptor.service_group.is_empty()).then_some(descriptor.service_group.as_str());
        self.stage_revision_files(&revision_dir, &bundle.files, group)
            .await?;
        let spec = RuntimeSpec {
            revision: manifest.config_rev,
            kernel_version: effective_kernel_version,
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
        if previous
            .as_ref()
            .is_some_and(|prior| prior.spec.revision == target.spec.revision)
        {
            self.apply_plan_locked(previous, target, Plan::Restart, op_id)
                .await?;
        } else {
            self.apply_prepared_locked(previous, target, op_id).await?;
        }
        Ok(manifest.config_rev)
    }
}

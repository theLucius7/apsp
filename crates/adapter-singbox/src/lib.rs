#![forbid(unsafe_code)]

mod health;
mod native;
mod obfuscation;
mod path_probe;
mod sentinel;
mod stats;
mod validation;
mod version;

use anyhow::{Context, Result, bail};
use apsp_adapter_sdk::{
    Adapter, BoxFuture, Descriptor, Plan, Prepared, Privileged, RuntimeProbeMeasurement,
    RuntimeSpec, ServiceManager, UsageSource,
};
use std::time::Duration;
use tokio::time::timeout;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(if cfg!(windows) { 30 } else { 10 });
const SERVICE_TIMEOUT: Duration = Duration::from_secs(if cfg!(windows) { 45 } else { 15 });
const SERVICE_QUERY_TIMEOUT: Duration = Duration::from_secs(if cfg!(windows) { 30 } else { 1 });
// A cold scheduled PowerShell process on Windows ARM64 can need nearly 30 seconds
// before the runtime starts. Allow startup and a complete service status query.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(if cfg!(windows) { 90 } else { 8 });
const SERVICE_UNIT: &str = "apsp-singbox@main.service";

#[derive(Clone, Copy, Debug, Default)]
pub struct SingboxAdapter;

impl SingboxAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn healthy_once(&self, target: &Prepared, services: &dyn ServiceManager) -> Result<bool> {
        if !timeout(SERVICE_QUERY_TIMEOUT, services.is_active(SERVICE_UNIT)).await?? {
            return Ok(false);
        }
        let addresses = native::listen_addresses(&target.spec)?;
        for address in addresses {
            if !health::probe(&address).await {
                return Ok(false);
            }
        }
        Ok(stats::query(&target.spec.stats_listen).await.is_ok())
    }
}

impl Adapter for SingboxAdapter {
    fn supports_runtime_probe(&self) -> bool {
        true
    }
    fn runtime_probe<'a>(
        &'a self,
        runtime: &'a Prepared,
        probe_id: &'a str,
    ) -> BoxFuture<'a, RuntimeProbeMeasurement> {
        Box::pin(path_probe::execute(runtime, probe_id))
    }

    fn supports_dependency_validation(&self) -> bool {
        true
    }
    fn validate_dependency<'a>(
        &'a self,
        runtime: &'a Prepared,
        scope: &'a str,
        generation: u64,
    ) -> BoxFuture<'a, ()> {
        Box::pin(validation::probe(runtime, scope, generation))
    }
    fn health_timeout(&self, target: &Prepared) -> Duration {
        health::budget(target) + Duration::from_secs(5)
    }

    fn describe(&self) -> Descriptor {
        Descriptor {
            module: "singbox".into(),
            plugin_name: "sing-box".into(),
            binary_name: if cfg!(windows) {
                "sing-box.exe"
            } else {
                "sing-box"
            }
            .into(),
            auxiliary_files: if cfg!(windows) {
                vec!["libcronet.dll".into()]
            } else {
                Vec::new()
            },
            service_unit: SERVICE_UNIT.into(),
            service_group: "apsp-singbox".into(),
        }
    }

    fn prepare<'a>(
        &'a self,
        runtime: RuntimeSpec,
        privileged: &'a dyn Privileged,
    ) -> BoxFuture<'a, Prepared> {
        Box::pin(async move {
            version::validate_requested(&runtime.kernel_version)?;
            if !runtime.binary_path.is_absolute() || !runtime.revision_dir.is_absolute() {
                bail!("runtime paths must be absolute");
            }
            let addresses = native::listen_addresses(&runtime)?;
            let config = runtime
                .files
                .get("config.json")
                .context("missing config.json")?;
            let config_path = runtime.revision_dir.join("config.json");
            if tokio::fs::read(&config_path)
                .await
                .context("read staged config")?
                != config.as_bytes()
            {
                bail!("staged configuration differs from the desired configuration");
            }
            if let Some(plan) = runtime.files.get(path_probe::PLAN_FILE)
                && tokio::fs::read(runtime.revision_dir.join(path_probe::PLAN_FILE))
                    .await
                    .context("read staged verification plan")?
                    != plan.as_bytes()
            {
                bail!("staged verification plan differs from the desired plan");
            }
            let version_args = ["version".into()];
            let output = timeout(
                COMMAND_TIMEOUT,
                privileged.execute(&runtime.binary_path, &version_args),
            )
            .await
            .context("runtime version command timed out")??;
            if !output.success {
                bail!("runtime version command failed");
            }
            version::validate_output(&output.stdout, &runtime.kernel_version)?;
            path_probe::validate_build(&runtime, &output.stdout)?;
            version::validate_features(
                &output.stdout,
                runtime.files.get("path-features.json").map(String::as_str),
            )?;
            let check_args = [
                "check".into(),
                "-c".into(),
                config_path
                    .to_str()
                    .context("config path is not UTF-8")?
                    .into(),
            ];
            let check = timeout(
                COMMAND_TIMEOUT,
                privileged.execute(&runtime.binary_path, &check_args),
            )
            .await
            .context("runtime config check timed out")??;
            if !check.success {
                // Native error messages can include credentials from the configuration.
                bail!("runtime configuration check failed");
            }
            Ok(Prepared {
                spec: runtime,
                listen_ports: addresses
                    .into_iter()
                    .map(|listener| listener.address.port())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            })
        })
    }

    fn plan<'a>(
        &'a self,
        previous: Option<&'a Prepared>,
        target: &'a Prepared,
    ) -> BoxFuture<'a, Plan> {
        Box::pin(async move {
            Ok(match previous {
                Some(previous) if previous.spec.kernel_version != target.spec.kernel_version => {
                    Plan::Restart
                }
                Some(previous) if previous.spec.config_hash == target.spec.config_hash => {
                    Plan::Noop
                }
                Some(_) if cfg!(windows) => Plan::Restart,
                Some(_) => Plan::Reload,
                None => Plan::Restart,
            })
        })
    }

    fn apply<'a>(
        &'a self,
        plan: Plan,
        target: &'a Prepared,
        services: &'a dyn ServiceManager,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            match plan {
                Plan::Noop => Ok(()),
                Plan::Restart => timeout(SERVICE_TIMEOUT, services.restart(SERVICE_UNIT))
                    .await
                    .context("runtime restart timed out")?,
                Plan::Reload => {
                    // Wait for the previous generation to close before core starts a new usage epoch.
                    let mut sentinel =
                        sentinel::Sentinel::connect(&target.spec.stats_listen).await?;
                    timeout(SERVICE_TIMEOUT, services.reload(SERVICE_UNIT))
                        .await
                        .context("runtime reload timed out")??;
                    sentinel.wait_closed().await
                }
            }
        })
    }

    fn health<'a>(
        &'a self,
        target: &'a Prepared,
        services: &'a dyn ServiceManager,
    ) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            native::listen_addresses(&target.spec)?;
            Ok(timeout(health::budget(target), async {
                loop {
                    if self.healthy_once(target, services).await.unwrap_or(false) {
                        return true;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            })
            .await
            .unwrap_or(false))
        })
    }

    fn usage_source(&self) -> Option<&dyn UsageSource> {
        Some(self)
    }
}

impl UsageSource for SingboxAdapter {
    fn read_counters<'a>(
        &'a self,
        runtime: &'a Prepared,
    ) -> BoxFuture<'a, Vec<apsp_adapter_sdk::Counter>> {
        Box::pin(async move {
            let users = stats::configured_users(&runtime.spec)?;
            let response = stats::query(&runtime.spec.stats_listen).await?;
            stats::counters(response, users)
        })
    }
}

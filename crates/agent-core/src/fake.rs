use apsp_adapter_sdk::{
    Adapter, BoxFuture, Counter, Descriptor, Plan, Prepared, Privileged, RuntimeSpec,
    ServiceManager, UsageSource,
};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

pub struct FakeResourceOps {
    pub inner: std::sync::Arc<dyn Privileged>,
    pub resources: Mutex<Result<apsp_adapter_sdk::DiagnosticResources, String>>,
}

impl FakeResourceOps {
    pub fn new(inner: std::sync::Arc<dyn Privileged>) -> Self {
        Self {
            inner,
            resources: Mutex::new(Ok(apsp_adapter_sdk::DiagnosticResources {
                memory: apsp_adapter_sdk::DiagnosticMemory {
                    host_available_bytes: 4 * 1024 * 1024 * 1024,
                    cgroup_available_bytes: None,
                },
                disk_available_bytes: 4 * 1024 * 1024 * 1024,
                load_one: 0.0,
                cpu_count: 2,
            })),
        }
    }
}

impl Privileged for FakeResourceOps {
    fn spawn_managed<'a>(
        &'a self,
        program: &'a std::path::Path,
        args: &'a [String],
    ) -> BoxFuture<'a, Box<dyn apsp_adapter_sdk::ManagedProcess>> {
        self.inner.spawn_managed(program, args)
    }
    fn execute_bounded<'a>(
        &'a self,
        program: &'a std::path::Path,
        args: &'a [String],
        timeout_secs: u32,
        maximum: usize,
    ) -> BoxFuture<'a, apsp_adapter_sdk::Execution> {
        self.inner
            .execute_bounded(program, args, timeout_secs, maximum)
    }
    fn install_archive_files<'a>(
        &'a self,
        path: &'a std::path::Path,
        directory: &'a std::path::Path,
        binary_name: &'a str,
        extras: &'a [String],
    ) -> BoxFuture<'a, ()> {
        self.inner
            .install_archive_files(path, directory, binary_name, extras)
    }
    fn diagnostic_memory(&self) -> BoxFuture<'_, apsp_adapter_sdk::DiagnosticMemory> {
        Box::pin(async {
            self.resources
                .lock()
                .unwrap()
                .clone()
                .map(|resources| resources.memory)
                .map_err(anyhow::Error::msg)
        })
    }
    fn diagnostic_resources<'a>(
        &'a self,
        _: &'a std::path::Path,
    ) -> BoxFuture<'a, apsp_adapter_sdk::DiagnosticResources> {
        Box::pin(async {
            self.resources
                .lock()
                .unwrap()
                .clone()
                .map_err(anyhow::Error::msg)
        })
    }
    fn execute<'a>(
        &'a self,
        path: &'a std::path::Path,
        args: &'a [String],
    ) -> BoxFuture<'a, apsp_adapter_sdk::CommandOutput> {
        self.inner.execute(path, args)
    }
    fn create_dir<'a>(
        &'a self,
        path: &'a std::path::Path,
        mode: u32,
        group: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        self.inner.create_dir(path, mode, group)
    }
    fn write_file<'a>(
        &'a self,
        path: &'a std::path::Path,
        bytes: &'a [u8],
        mode: u32,
        group: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        self.inner.write_file(path, bytes, mode, group)
    }
    fn atomic_symlink<'a>(
        &'a self,
        link: &'a std::path::Path,
        target: &'a std::path::Path,
    ) -> BoxFuture<'a, ()> {
        self.inner.atomic_symlink(link, target)
    }
    fn remove_symlink<'a>(&'a self, path: &'a std::path::Path) -> BoxFuture<'a, ()> {
        self.inner.remove_symlink(path)
    }
    fn remove_file<'a>(&'a self, path: &'a std::path::Path) -> BoxFuture<'a, ()> {
        self.inner.remove_file(path)
    }
    fn remove_managed_directory<'a>(&'a self, path: &'a std::path::Path) -> BoxFuture<'a, ()> {
        self.inner.remove_managed_directory(path)
    }
    fn install_archive<'a>(
        &'a self,
        archive: &'a std::path::Path,
        directory: &'a std::path::Path,
        name: &'a str,
    ) -> BoxFuture<'a, ()> {
        self.inner.install_archive(archive, directory, name)
    }
}

#[derive(Default)]
pub struct FakeServiceManager {
    pub active: AtomicBool,
    pub fail_next: AtomicBool,
    pub actions: Mutex<Vec<String>>,
}

impl FakeServiceManager {
    fn action(&self, action: &str, unit: &str, active: bool) -> anyhow::Result<()> {
        self.actions
            .lock()
            .unwrap()
            .push(format!("{action}:{unit}"));
        anyhow::ensure!(
            !self.fail_next.swap(false, Ordering::SeqCst),
            "injected service failure"
        );
        self.active.store(active, Ordering::SeqCst);
        Ok(())
    }
}

impl ServiceManager for FakeServiceManager {
    fn reload<'a>(&'a self, unit: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move { self.action("reload", unit, true) })
    }
    fn restart<'a>(&'a self, unit: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move { self.action("restart", unit, true) })
    }
    fn stop<'a>(&'a self, unit: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move { self.action("stop", unit, false) })
    }
    fn is_active<'a>(&'a self, _unit: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move { Ok(self.active.load(Ordering::SeqCst)) })
    }
}

#[derive(Default)]
pub struct FakeAdapter {
    pub counters: Mutex<Vec<Counter>>,
    pub applied: Mutex<Vec<u64>>,
    pub fail_prepare: AtomicBool,
    pub fail_apply: AtomicBool,
    pub fail_health: AtomicBool,
    pub fail_counters: AtomicBool,
    pub apply_delay_ms: AtomicU64,
    pub next_health_delay_ms: AtomicU64,
    pub health_budget_secs: AtomicU64,
    pub probe_supported: AtomicBool,
    pub probe_delay_ms: AtomicU64,
    pub fail_probe: AtomicBool,
    pub probe_calls: Mutex<Vec<String>>,
    pub probe_started: tokio::sync::Notify,
}

impl Adapter for FakeAdapter {
    fn supports_runtime_probe(&self) -> bool {
        self.probe_supported.load(Ordering::SeqCst)
    }
    fn runtime_probe<'a>(
        &'a self,
        _runtime: &'a Prepared,
        id: &'a str,
    ) -> BoxFuture<'a, apsp_adapter_sdk::RuntimeProbeMeasurement> {
        Box::pin(async move {
            self.probe_calls.lock().unwrap().push(id.into());
            self.probe_started.notify_one();
            tokio::time::sleep(Duration::from_millis(
                self.probe_delay_ms.load(Ordering::SeqCst),
            ))
            .await;
            anyhow::ensure!(
                !self.fail_probe.load(Ordering::SeqCst),
                "private fixture credential must never escape"
            );
            Ok(apsp_adapter_sdk::RuntimeProbeMeasurement { elapsed_ms: 17 })
        })
    }
    fn health_timeout(&self, _target: &Prepared) -> Duration {
        Duration::from_secs(self.health_budget_secs.load(Ordering::SeqCst))
    }
    fn describe(&self) -> Descriptor {
        Descriptor {
            module: "demo".into(),
            plugin_name: "demo".into(),
            binary_name: "demo".into(),
            auxiliary_files: Vec::new(),
            service_unit: "demo@main".into(),
            service_group: String::new(),
        }
    }
    fn prepare<'a>(
        &'a self,
        runtime: RuntimeSpec,
        _privileged: &'a dyn Privileged,
    ) -> BoxFuture<'a, Prepared> {
        Box::pin(async move {
            anyhow::ensure!(
                !self.fail_prepare.swap(false, Ordering::SeqCst),
                "injected preparation failure"
            );
            Ok(Prepared {
                spec: runtime,
                listen_ports: vec![],
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
                Some(previous) if previous.spec.kernel_version == target.spec.kernel_version => {
                    if previous.spec.config_hash == target.spec.config_hash {
                        Plan::Noop
                    } else {
                        Plan::Reload
                    }
                }
                _ => Plan::Restart,
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
            self.applied.lock().unwrap().push(target.spec.revision);
            tokio::time::sleep(Duration::from_millis(
                self.apply_delay_ms.load(Ordering::SeqCst),
            ))
            .await;
            anyhow::ensure!(
                !self.fail_apply.swap(false, Ordering::SeqCst),
                "injected application failure"
            );
            match plan {
                Plan::Noop => {}
                Plan::Reload => services.reload("demo@main").await?,
                Plan::Restart => services.restart("demo@main").await?,
            }
            if plan != Plan::Noop {
                for counter in self.counters.lock().unwrap().iter_mut() {
                    counter.uplink = 0;
                    counter.downlink = 0;
                }
            }
            Ok(())
        })
    }
    fn health<'a>(
        &'a self,
        _target: &'a Prepared,
        services: &'a dyn ServiceManager,
    ) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(
                self.next_health_delay_ms.swap(0, Ordering::SeqCst),
            ))
            .await;
            if self.fail_health.swap(false, Ordering::SeqCst) {
                return Ok(false);
            }
            services.is_active("demo@main").await
        })
    }
    fn usage_source(&self) -> Option<&dyn UsageSource> {
        Some(self)
    }
}

impl UsageSource for FakeAdapter {
    fn read_counters<'a>(&'a self, _runtime: &'a Prepared) -> BoxFuture<'a, Vec<Counter>> {
        Box::pin(async move {
            anyhow::ensure!(
                !self.fail_counters.swap(false, Ordering::SeqCst),
                "injected counter failure"
            );
            Ok(self.counters.lock().unwrap().clone())
        })
    }
}

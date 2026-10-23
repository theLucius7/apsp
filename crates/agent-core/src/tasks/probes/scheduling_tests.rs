use super::*;
use apsp_adapter_sdk::{BoxFuture, CommandOutput};
use apsp_protocol::{
    AuthorizedProbe, ProbeAddressFamily, ProbeAuthorization, ProbeAuthorizationKind, ProbeLease,
    ProbeMonitor, ProbeNetwork,
};
use std::{
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Default)]
pub(super) struct ControlledOps {
    pub(super) active: AtomicUsize,
    pub(super) peak: AtomicUsize,
    pub(super) starts: AtomicUsize,
    pub(super) release: tokio::sync::Notify,
}

struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Privileged for ControlledOps {
    fn execute<'a>(&'a self, _: &'a Path, args: &'a [String]) -> BoxFuture<'a, CommandOutput> {
        Box::pin(async move {
            self.starts.fetch_add(1, Ordering::SeqCst);
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            let _active = Active(&self.active);
            if !args.last().unwrap().contains("127.0.0.1") {
                self.release.notified().await;
            }
            Ok(CommandOutput {
                success: true,
                stdout: if cfg!(windows) {
                    r#"{"sent":4,"times":[0,0,0,0]}"#
                } else {
                    "4 packets transmitted, 4 received\nrtt min/avg/max = 0/0/0 ms"
                }
                .into(),
                stderr: String::new(),
            })
        })
    }
    fn create_dir<'a>(&'a self, _: &'a Path, _: u32, _: Option<&'a str>) -> BoxFuture<'a, ()> {
        Box::pin(async { anyhow::bail!("unexpected filesystem operation") })
    }
    fn write_file<'a>(
        &'a self,
        _: &'a Path,
        _: &'a [u8],
        _: u32,
        _: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async { anyhow::bail!("unexpected filesystem operation") })
    }
    fn atomic_symlink<'a>(&'a self, _: &'a Path, _: &'a Path) -> BoxFuture<'a, ()> {
        Box::pin(async { anyhow::bail!("unexpected filesystem operation") })
    }
    fn remove_symlink<'a>(&'a self, _: &'a Path) -> BoxFuture<'a, ()> {
        Box::pin(async { anyhow::bail!("unexpected filesystem operation") })
    }
    fn install_archive<'a>(&'a self, _: &'a Path, _: &'a Path, _: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async { anyhow::bail!("unexpected filesystem operation") })
    }
}

pub(super) async fn until(condition: impl Fn() -> bool) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("probe scheduler did not make progress")
}

pub(super) fn authorize(mut spec: ProbeSpec) -> ProbeSpec {
    if spec.monitor.is_none() {
        let identity = spec.identity();
        spec.monitor = Some(ProbeMonitor {
            network: ProbeNetwork::Other,
            region: "local fixture".into(),
            address_family: ProbeAddressFamily::Any,
            authorization: Some(ProbeAuthorization {
                kind: ProbeAuthorizationKind::Owned,
                source: "operator-owned loopback".into(),
                scope: "controlled test fixture".into(),
                enabled: true,
                expires_at: None,
                identity,
            }),
        });
    }
    spec
}

pub(super) fn accepted(
    session: &Arc<PanelClient>,
    specs: &[ProbeSpec],
    revision: u64,
    duration: Duration,
) -> AcceptedLease {
    let now = Instant::now();
    let issued_at = now_timestamp();
    AcceptedLease {
        snapshot: ProbeLease {
            id: Uuid::new_v4(),
            server_id: 7,
            revision,
            issued_at,
            expires_at: issued_at + 90,
            probes: specs
                .iter()
                .cloned()
                .map(|spec| AuthorizedProbe {
                    authorization: spec
                        .monitor
                        .as_ref()
                        .unwrap()
                        .authorization
                        .clone()
                        .unwrap(),
                    spec,
                })
                .collect(),
        },
        received: now,
        deadline: now + duration,
        session: session.clone(),
    }
}

#[tokio::test]
async fn slow_probes_do_not_delay_results_and_configuration_changes_cancel_work() -> Result<()> {
    let state = Arc::new(Mutex::new(crate::State::open(Path::new(":memory:"))?));
    let ops = Arc::new(ControlledOps::default());
    let retirement = Arc::new(crate::retirement::Retirement::new(
        crate::Config::default(),
        state.clone(),
        vec![],
        ops.clone(),
        Arc::new(crate::fake::FakeServiceManager::default()),
    )?);
    let mut specs: Vec<_> = (1..=6)
        .map(|id| ProbeSpec {
            id: Uuid::from_u128(id),
            name: format!("fixture-{id}"),
            kind: ProbeKind::Icmp,
            target: if id == 1 { "127.0.0.1" } else { "127.0.0.2" }.into(),
            port: None,
            interval_secs: 3600,
            carrier: String::new(),
            enabled: true,
            monitor: None,
            execution_authorized: None,
        })
        .map(authorize)
        .collect();
    let client = Arc::new(PanelClient::new("http://127.0.0.1:1", "fixture-session")?);
    let (_clients, clients) = watch::channel(Some(client.clone()));
    let (leases, lease_receiver) =
        watch::channel(Some(accepted(&client, &specs, 1, Duration::from_secs(90))));
    let worker = tokio::spawn(sample_loop(
        state.clone(),
        ops.clone(),
        clients,
        lease_receiver,
        retirement.clone(),
    ));
    until(|| state.lock().unwrap().probe_results().unwrap().len() == 1).await?;
    until(|| ops.active.load(Ordering::SeqCst) == 4).await?;
    // Remove a slow target and let a new ready target take the freed slot,
    // without shortening the already completed target's persisted interval.
    specs.retain(|spec| ![Uuid::from_u128(2), Uuid::from_u128(6)].contains(&spec.id));
    let mut next = specs[0].clone();
    next.id = Uuid::from_u128(7);
    specs.push(next);
    leases.send_replace(Some(accepted(&client, &specs, 2, Duration::from_secs(90))));
    until(|| state.lock().unwrap().probe_results().unwrap().len() == 2).await?;
    assert!(ops.peak.load(Ordering::SeqCst) <= 4);
    leases.send_replace(Some(accepted(&client, &[], 3, Duration::from_secs(90))));
    until(|| ops.active.load(Ordering::SeqCst) == 0).await?;
    // Cancelled measurements release the gate used by retirement and persist no results.
    let guard = timeout(Duration::from_secs(1), retirement.gate.write()).await?;
    assert_eq!(state.lock().unwrap().probe_results()?.len(), 2);
    drop(guard);
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    Ok(())
}

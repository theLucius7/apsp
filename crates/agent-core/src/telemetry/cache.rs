mod cancellation;

use super::{Collector, hardware};
use anyhow::{Context, Result};
use apsp_adapter_sdk::Privileged;
use apsp_protocol::{Metrics, StaticInfo, TelemetrySample, telemetry::now_millis};
use cancellation::Cancellation;
use std::{
    collections::BTreeMap,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use tokio::sync::watch;
use uuid::Uuid;

const COLLECTION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(crate) struct Snapshot {
    pub static_info: StaticInfo,
    pub sample: Option<TelemetrySample>,
    pub started_at: Option<Instant>,
    pub error: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            static_info: StaticInfo {
                os: Some(std::env::consts::OS.into()),
                libc: cfg!(target_os = "linux").then(|| {
                    if cfg!(target_env = "musl") {
                        "musl"
                    } else {
                        "gnu"
                    }
                    .into()
                }),
                arch: Some(std::env::consts::ARCH.into()),
                ..StaticInfo::default()
            },
            sample: None,
            started_at: None,
            error: None,
        }
    }
}

impl Snapshot {
    pub fn timed_out(&self) -> bool {
        self.started_at
            .is_some_and(|time| time.elapsed() >= COLLECTION_TIMEOUT)
    }
}

#[derive(Clone)]
pub(crate) struct Control {
    pub interval: Duration,
    pub offset_ms: i64,
    pub minimum_timestamp: i64,
    pub enabled: bool,
}

pub(crate) trait Source {
    fn collect(&mut self) -> Result<(StaticInfo, Metrics)>;
}

struct NativeSource {
    collector: Collector,
    ops: Arc<dyn Privileged>,
    runtime: tokio::runtime::Runtime,
    previous: (Instant, BTreeMap<String, [u64; 7]>),
    supplement: hardware::Hardware,
    refreshed: Option<Instant>,
    cancellation: Arc<Cancellation>,
}

impl Source for NativeSource {
    fn collect(&mut self) -> Result<(StaticInfo, Metrics)> {
        if self
            .refreshed
            .is_none_or(|time| time.elapsed() >= Duration::from_secs(5))
        {
            self.supplement = self.cancellation.run(
                &self.runtime,
                hardware::refresh(self.ops.as_ref(), &mut self.previous),
            )?;
            self.refreshed = Some(Instant::now());
        }
        anyhow::ensure!(
            !self.cancellation.requested(),
            "telemetry collection stopped"
        );
        let mut metrics = self.collector.metrics();
        self.supplement.apply(&mut metrics);
        Ok((self.collector.static_info(), metrics))
    }
}

pub(crate) struct Sampling {
    // Keep the last snapshot available even if the collecting thread panics.
    _snapshot: watch::Sender<Arc<Snapshot>>,
    pub snapshots: watch::Receiver<Arc<Snapshot>>,
    pub control: watch::Sender<Control>,
    cancellation: Arc<Cancellation>,
    wake: mpsc::Sender<()>,
}

impl Sampling {
    pub fn start(ops: Arc<dyn Privileged>, control: Control) -> Result<Self> {
        Self::start_with_cancellation(
            move |cancellation| {
                Ok(Box::new(NativeSource {
                    collector: Collector::new(),
                    ops,
                    runtime: tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?,
                    previous: (Instant::now(), BTreeMap::new()),
                    supplement: Default::default(),
                    refreshed: None,
                    cancellation,
                }))
            },
            control,
        )
    }

    #[cfg(test)]
    pub(crate) fn start_with(
        factory: impl FnOnce() -> Result<Box<dyn Source>> + Send + 'static,
        control: Control,
    ) -> Result<Self> {
        Self::start_with_cancellation(move |_| factory(), control)
    }

    fn start_with_cancellation(
        factory: impl FnOnce(Arc<Cancellation>) -> Result<Box<dyn Source>> + Send + 'static,
        control: Control,
    ) -> Result<Self> {
        let (snapshot_tx, snapshots) = watch::channel(Arc::new(Snapshot::default()));
        let (control_tx, control_rx) = watch::channel(control);
        let cancellation = Arc::new(Cancellation::default());
        let (wake, wait) = mpsc::channel();
        let stop = cancellation.clone();
        let output = snapshot_tx.clone();
        // Exactly one thread owns the collector for this Agent process. A timeout
        // reports stale data; it never starts another possibly blocked collector.
        std::thread::Builder::new()
            .name("apsp-telemetry".into())
            .spawn(move || {
                let mut cached = Snapshot {
                    started_at: Some(Instant::now()),
                    ..Snapshot::default()
                };
                output.send_replace(Arc::new(cached.clone()));
                let mut source = match factory(stop.clone()) {
                    Ok(source) => source,
                    Err(error) => {
                        cached.error = Some(error.to_string());
                        output.send_replace(Arc::new(cached));
                        return;
                    }
                };
                let mut last_timestamp: i64 = 0;
                while !stop.requested() {
                    let control = control_rx.borrow().clone();
                    if control.enabled {
                        let started = Instant::now();
                        let timestamp = now_millis()
                            .saturating_add(control.offset_ms)
                            .max(last_timestamp.saturating_add(1))
                            .max(control.minimum_timestamp.saturating_add(1));
                        cached.started_at = Some(started);
                        output.send_replace(Arc::new(cached.clone()));
                        let collected = source.collect();
                        if stop.requested() {
                            break;
                        }
                        match collected {
                            Ok((info, metrics)) if started.elapsed() < COLLECTION_TIMEOUT => {
                                last_timestamp = timestamp;
                                cached.static_info = info;
                                cached.sample = Some(TelemetrySample {
                                    id: Uuid::new_v4(),
                                    sampled_at: timestamp,
                                    metrics,
                                });
                                cached.error = None;
                            }
                            Ok(_) => {
                                cached.error =
                                    Some("telemetry collection exceeded five seconds".into())
                            }
                            Err(error) => cached.error = Some(error.to_string()),
                        }
                        cached.started_at = None;
                        output.send_replace(Arc::new(cached.clone()));
                    }
                    if wait.recv_timeout(control.interval).is_ok() {
                        break;
                    }
                }
            })
            .context("start isolated telemetry thread")?;
        Ok(Self {
            _snapshot: snapshot_tx,
            snapshots,
            control: control_tx,
            cancellation,
            wake,
        })
    }
}

impl Drop for Sampling {
    fn drop(&mut self) {
        self.cancellation.stop();
        let _ = self.wake.send(());
        // A synchronous collector may be stuck in a kernel call; never join it.
    }
}

#[cfg(test)]
pub(crate) mod tests;

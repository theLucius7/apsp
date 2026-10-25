//! Applies concurrent client address limits to the applied runtime's sessions.
use super::Runtime;
use crate::reconcile::Reconciler;
use anyhow::Result;
use apsp_protocol::RuntimeAdmissionPolicy;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone)]
pub(super) struct Admission {
    sender: mpsc::Sender<RuntimeAdmissionPolicy>,
}

impl Admission {
    pub(super) fn channel() -> (Self, mpsc::Receiver<RuntimeAdmissionPolicy>) {
        let (sender, receiver) = mpsc::channel(4);
        (Self { sender }, receiver)
    }

    /// WebSocket processing never waits; the panel repeats policies regularly.
    pub(super) fn enqueue(&self, policy: RuntimeAdmissionPolicy) {
        if self.sender.try_send(policy).is_err() {
            tracing::debug!("admission policy queue is full; the next policy replaces it");
        }
    }
}

/// The module whose runtime both reports sessions and logs into the
/// volatile runtime namespace, if any.
pub(super) async fn supported(
    reconcilers: &[(String, Arc<Reconciler>)],
) -> Option<(String, Arc<Reconciler>)> {
    #[cfg(target_os = "linux")]
    for (module, reconciler) in reconcilers {
        if reconciler.session_source().is_some()
            && (crate::admission::journal::available(&reconciler.descriptor().service_unit).await
                || crate::runtime_logs::available(&reconciler.descriptor().service_unit))
        {
            return Some((module.clone(), reconciler.clone()));
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = reconcilers;
    None
}

#[cfg(not(target_os = "linux"))]
pub(super) async fn run(
    _module: String,
    _reconciler: Arc<Reconciler>,
    _runtime: Runtime,
    mut policies: mpsc::Receiver<RuntimeAdmissionPolicy>,
    _outgoing: mpsc::Sender<apsp_protocol::Envelope>,
) -> Result<()> {
    while policies.recv().await.is_some() {}
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) async fn run(
    module: String,
    reconciler: Arc<Reconciler>,
    runtime: Runtime,
    mut policies: mpsc::Receiver<RuntimeAdmissionPolicy>,
    outgoing: mpsc::Sender<apsp_protocol::Envelope>,
) -> Result<()> {
    use crate::admission::{Engine, journal};
    use apsp_protocol::{Envelope, RUNTIME_SESSION_REPORT_KIND, now_timestamp};
    use std::{
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };
    use tokio::time::timeout;

    const LINES_PER_TICK: usize = 20_000;
    let unit = reconciler.descriptor().service_unit;
    let (line_sender, mut lines) = mpsc::channel::<String>(LINES_PER_TICK);
    let mut follower: Option<tokio::task::JoinHandle<Result<()>>> = None;
    let mut engine = Engine::default();
    let mut applied: Option<(u64, String)> = None;
    let mut last_report: Option<(Instant, apsp_protocol::RuntimeSessionReport)> = None;
    // Recover attributions of sessions opened before an Agent restart.
    let mut since = "-10min";
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            policy = policies.recv() => {
                let Some(policy) = policy else { return Ok(()) };
                if policy.module == module && !engine.set_policy(&policy, Instant::now()) {
                    tracing::warn!("ignored an invalid admission policy");
                }
                continue;
            }
            _ = tick.tick() => {}
        }
        if runtime
            .retirement
            .as_ref()
            .is_some_and(|retirement| retirement.requested())
        {
            continue;
        }
        let Some(source) = reconciler.session_source() else {
            return Ok(());
        };
        let prepared = match reconciler.applied_runtime() {
            Ok(Some(prepared)) if source.enabled(&prepared) => prepared,
            Ok(_) => {
                if let Some(task) = follower.take() {
                    task.abort();
                }
                engine.reset();
                applied = None;
                continue;
            }
            Err(error) => {
                tracing::debug!(%error, "applied runtime is unreadable; session control waits");
                continue;
            }
        };
        let identity = (prepared.spec.revision, prepared.spec.config_hash.clone());
        if applied.as_ref() != Some(&identity) {
            // Applying a configuration restarts the runtime and its sessions.
            engine.reset();
            applied = Some(identity);
        }
        if follower.as_ref().is_none_or(|task| task.is_finished()) {
            let sender = line_sender.clone();
            let (unit, start) = (unit.clone(), since.to_owned());
            follower = Some(tokio::spawn(async move {
                if crate::runtime_logs::available(&unit) {
                    crate::runtime_logs::follow(&unit, &start, sender).await
                } else {
                    journal::follow(&unit, &start, sender).await
                }
            }));
            since = "-2min";
        }
        let now = Instant::now();
        for _ in 0..LINES_PER_TICK {
            let Ok(line) = lines.try_recv() else { break };
            if let Some(evidence) = source.parse_log_line(&line) {
                engine.evidence(evidence, now);
            }
        }
        let active = match timeout(Duration::from_secs(5), source.active_sessions(&prepared)).await
        {
            Ok(Ok(active)) => active,
            Ok(Err(error)) => {
                tracing::debug!(%error, "runtime sessions are unavailable");
                continue;
            }
            Err(_) => continue,
        };
        let decision = engine.tick(&active, now);
        if !decision.close.is_empty() {
            match timeout(
                Duration::from_secs(10),
                source.close_sessions(&prepared, &decision.close),
            )
            .await
            {
                Ok(Ok(closed)) => tracing::info!(
                    closed,
                    refused = decision.close.len(),
                    "closed sessions beyond a concurrent client address limit"
                ),
                Ok(Err(error)) => tracing::warn!(%error, "refused sessions could not be closed"),
                Err(_) => tracing::warn!("closing refused sessions timed out"),
            }
        }
        let report = engine.report(&module, now_timestamp());
        let due = last_report.as_ref().is_none_or(|(at, previous)| {
            at.elapsed() >= Duration::from_secs(10)
                || (previous.principals != report.principals
                    && at.elapsed() >= Duration::from_secs(2))
        });
        if due
            && runtime.connected.load(Ordering::Relaxed)
            && outgoing
                .try_send(Envelope::new(RUNTIME_SESSION_REPORT_KIND, &report)?)
                .is_ok()
        {
            last_report = Some((now, report));
        }
    }
}

use super::*;
use apsp_protocol::{DiagnosticCancelRequest, DiagnosticCancelResult, Envelope};
use tokio::sync::mpsc;

mod actions;
pub(super) const PENDING: &str = "diagnostics:cancellations";
const RESULTS: &str = "diagnostics:cancellation-results";

pub struct CancellationControl {
    state: SharedState,
    server_id: i64,
    plugins: Vec<String>,
    wake: watch::Sender<u64>,
    outgoing: Option<mpsc::Sender<Envelope>>,
}

impl CancellationControl {
    pub fn new(state: SharedState, server_id: i64, plugins: Vec<String>) -> Self {
        Self {
            state,
            server_id,
            plugins,
            wake: watch::channel(0).0,
            outgoing: None,
        }
    }
    pub fn with_outgoing(mut self, outgoing: mpsc::Sender<Envelope>) -> Self {
        self.outgoing = Some(outgoing);
        self
    }
    pub(super) fn subscribe(&self) -> watch::Receiver<u64> {
        self.wake.subscribe()
    }

    pub fn request(&self, request: DiagnosticCancelRequest) -> Result<()> {
        ensure!(
            request.server_id == self.server_id
                && self.plugins.contains(&request.job.plugin)
                && !request.job.id.is_nil()
                && safe_component(&request.job.version)
                && (1..=3600).contains(&request.job.timeout_secs),
            "cancel request does not match this device or a registered plugin"
        );
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        let results: Vec<DiagnosticCancelResult> = state.get_json(RESULTS)?.unwrap_or_default();
        if results
            .iter()
            .any(|result| result.id == request.job.id && result.confirmed)
        {
            return Ok(());
        }
        let active: Option<Checkpoint> = state.get_json::<Option<Checkpoint>>(ACTIVE)?.flatten();
        if let Some(Checkpoint::Started { spec, plugin, .. }) = active
            && spec.id == request.job.id.to_string()
        {
            ensure!(
                plugin == request.job.plugin && spec.version == request.job.version,
                "cancel request differs from the known diagnostic identity"
            );
        }
        let mut requests: Vec<DiagnosticCancelRequest> =
            state.get_json(PENDING)?.unwrap_or_default();
        if let Some(known) = requests.iter().find(|known| known.job.id == request.job.id) {
            ensure!(
                known == &request,
                "duplicate cancellation changed the known task"
            );
            return Ok(());
        }
        ensure!(
            requests.len() < 64,
            "too many pending diagnostic cancellations"
        );
        requests.push(request);
        state.set_json(PENDING, &requests)?;
        drop(state);
        self.wake
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        Ok(())
    }

    fn pending(&self) -> Result<Vec<DiagnosticCancelRequest>> {
        Ok(self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
            .get_json(PENDING)?
            .unwrap_or_default())
    }
    pub(super) fn requested(&self, id: Uuid) -> Result<bool> {
        Ok(self.pending()?.iter().any(|request| request.job.id == id))
    }

    fn save_result(&self, result: DiagnosticCancelResult) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        let mut results: Vec<DiagnosticCancelResult> = state.get_json(RESULTS)?.unwrap_or_default();
        if results
            .iter()
            .any(|saved| saved.id == result.id && saved.confirmed)
        {
            return Ok(());
        }
        results.retain(|saved| saved.id != result.id);
        ensure!(
            results.len() < 64,
            "too many unacknowledged cancellation results"
        );
        results.push(result.clone());
        let mut changes = vec![(RESULTS.into(), serde_json::to_value(results)?)];
        if result.confirmed {
            let mut requests: Vec<DiagnosticCancelRequest> =
                state.get_json(PENDING)?.unwrap_or_default();
            requests.retain(|request| request.job.id != result.id);
            changes.push((PENDING.into(), serde_json::to_value(requests)?));
            changes.push((
                format!("diagnostics:done:{}", result.id),
                serde_json::Value::Bool(true),
            ));
            let active: Option<Checkpoint> =
                state.get_json::<Option<Checkpoint>>(ACTIVE)?.flatten();
            let matches = match active {
                Some(Checkpoint::Preparing(job)) => job.id == result.id,
                Some(Checkpoint::Started { spec, .. }) => spec.id == result.id.to_string(),
                None => false,
            };
            if matches {
                changes.push((ACTIVE.into(), serde_json::Value::Null));
            }
        }
        state.set_json_batch(&changes)?;
        drop(state);
        if let Some(outgoing) = &self.outgoing {
            let _ = outgoing.try_send(Envelope::new("diagnostic.cancel.result", result)?);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn flush(&self, client: &PanelClient) -> Result<()> {
        self.flush_guarded(client, None).await
    }

    async fn flush_guarded(
        &self,
        client: &PanelClient,
        retirement: Option<&crate::retirement::Retirement>,
    ) -> Result<()> {
        let results: Vec<DiagnosticCancelResult> = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
            .get_json(RESULTS)?
            .unwrap_or_default();
        for result in results {
            if retirement.is_some_and(|retirement| retirement.requested()) {
                return Ok(());
            }
            tokio::time::timeout(
                Duration::from_secs(5),
                client.diagnostic_cancel_result(&result),
            )
            .await
            .context("cancellation acknowledgment timed out")??;
            if retirement.is_some_and(|retirement| retirement.requested()) {
                return Ok(());
            }
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
            let mut pending: Vec<DiagnosticCancelResult> =
                state.get_json(RESULTS)?.unwrap_or_default();
            // A stale negative acknowledgment must not remove a later positive result.
            pending.retain(|saved| saved != &result);
            state.set_json(RESULTS, &pending)?;
        }
        Ok(())
    }

    pub(crate) async fn run(
        self: Arc<Self>,
        mut client: watch::Receiver<Option<Arc<PanelClient>>>,
        retirement: Arc<crate::retirement::Retirement>,
    ) -> Result<()> {
        let mut poll = tokio::time::interval(Duration::from_secs(5));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = poll.tick() => {},
                changed = client.changed() => { if changed.is_err() { return Ok(()); } },
            }
            // Retirement clears these keys under its write gate. Keep this round
            // inside the read gate, but release it before polling/sleeping again.
            let _guard = retirement.gate.read().await;
            if retirement.requested() {
                continue;
            }
            let current = client.borrow().clone();
            let Some(current) = current else {
                continue;
            };
            let attempt: Result<()> = async {
                self.flush_guarded(&current, Some(&retirement)).await?;
                if retirement.requested() {
                    return Ok(());
                }
                let pending = tokio::time::timeout(
                    Duration::from_secs(5),
                    current.diagnostic_cancellations(),
                )
                .await
                .context("pending cancellation query timed out")??;
                for request in pending {
                    if retirement.requested() {
                        break;
                    }
                    self.request(request)?;
                }
                Ok(())
            }
            .await;
            if let Err(error) = attempt {
                tracing::warn!(%error, "cancellation HTTP recovery will retry");
            }
        }
    }
}

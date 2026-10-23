use super::Runtime;
use crate::{
    reconcile::Reconciler,
    state::runtime_control::{ControlRequest, ControlResult},
};
use anyhow::Result;
use apsp_protocol::{
    Envelope, RuntimeCheckpointResult, RuntimeControlAck, RuntimePathProbeResult,
    RuntimeRecoveryBarrierResult,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;

pub(super) enum Input {
    Request(ControlRequest),
    Ack(&'static str, RuntimeControlAck),
}

#[derive(Clone)]
pub(super) struct Control {
    sender: mpsc::Sender<Input>,
}

impl Control {
    pub(super) fn channel() -> (Self, mpsc::Receiver<Input>) {
        let (sender, receiver) = mpsc::channel(64);
        (Self { sender }, receiver)
    }
    /// WebSocket processing must never wait for SQLite, the apply gate or health.
    pub(super) fn enqueue(&self, input: Input) {
        if self.sender.try_send(input).is_err() {
            tracing::warn!(
                "runtime control queue is full or closed; panel must retry the same request"
            );
        }
    }
}

pub(super) async fn run(
    reconcilers: Vec<(String, Arc<Reconciler>)>,
    runtime: Runtime,
    mut receiver: mpsc::Receiver<Input>,
    outgoing: mpsc::Sender<Envelope>,
) -> Result<()> {
    let mut retry = tokio::time::interval(Duration::from_secs(5));
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut cursor = 0;
    loop {
        let input = tokio::select! {
            input = receiver.recv() => match input { Some(input) => Some(input), None => return Ok(()) },
            _ = retry.tick() => None,
        };
        let _retirement_gate = match &runtime.retirement {
            Some(retirement) => Some(retirement.gate.read().await),
            None => None,
        };
        if runtime
            .retirement
            .as_ref()
            .is_some_and(|retirement| retirement.requested())
        {
            continue;
        }
        if let Some(input) = input {
            let outcome: Result<Option<ControlResult>> = (|| {
                let mut state = runtime
                    .state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state poisoned"))?;
                match input {
                    Input::Request(request) => state.enqueue_runtime_control(&request),
                    Input::Ack(kind, ack) => {
                        state.acknowledge_runtime_control(kind, &ack)?;
                        Ok(None)
                    }
                }
            })();
            match outcome {
                Ok(Some(result)) => {
                    let _ = outgoing.try_send(result.envelope()?);
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%error, "runtime control input rejected; durable work was preserved")
                }
            }
        }
        let pending = runtime
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?
            .pending_runtime_controls()?;
        for request in pending {
            let result = if let Some((_, reconciler)) = reconcilers
                .iter()
                .find(|(module, _)| module == request.module())
            {
                reconciler.runtime_control(&request).await
            } else {
                let result = unsupported(&request)?;
                runtime
                    .state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state poisoned"))?
                    .finish_runtime_control(&result, None)?;
                Ok(result)
            };
            match result {
                Ok(result) => {
                    let _ = outgoing.try_send(result.envelope()?);
                }
                Err(error) => {
                    tracing::warn!(%error, "runtime control execution failed; request retained")
                }
            }
        }
        if runtime.connected.load(std::sync::atomic::Ordering::Relaxed) {
            let mut saved = runtime
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("state poisoned"))?
                .runtime_results_after(cursor)?;
            if saved.is_empty() && cursor != 0 {
                cursor = 0;
                saved = runtime
                    .state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state poisoned"))?
                    .runtime_results_after(cursor)?;
            }
            for (position, result) in saved {
                if outgoing.try_send(result.envelope()?).is_err() {
                    break;
                }
                cursor = position;
            }
        }
    }
}

fn unsupported(request: &ControlRequest) -> Result<ControlResult> {
    let error = Some("requested runtime module is not registered on this device".into());
    let request_digest = request.digest()?;
    Ok(match request {
        ControlRequest::Checkpoint(value) => ControlResult::Checkpoint(RuntimeCheckpointResult {
            request_id: value.request_id,
            request_digest,
            observed: None,
            success: false,
            error,
        }),
        ControlRequest::Probe(value) => ControlResult::Probe(RuntimePathProbeResult {
            request_id: value.request_id,
            request_digest,
            observed: None,
            probe_id: value.probe_id,
            elapsed_ms: None,
            success: false,
            error,
        }),
        ControlRequest::Barrier(value) => ControlResult::Barrier(RuntimeRecoveryBarrierResult {
            request_id: value.request_id,
            request_digest,
            observed: None,
            minimum_revision: None,
            pending_intents_clear: false,
            success: false,
            error,
        }),
    })
}

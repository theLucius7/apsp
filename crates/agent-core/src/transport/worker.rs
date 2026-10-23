use super::Runtime;
use crate::{artifacts::PanelClient, reconcile::Reconciler, state::StorageRetry};
use anyhow::Result;
use apsp_protocol::{ApplyResult, ApplyStatus, Envelope};
use std::{sync::Arc, time::Duration};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

pub(super) async fn run(
    reconcilers: Vec<(String, Arc<Reconciler>)>,
    runtime: Runtime,
    client: watch::Receiver<Option<Arc<PanelClient>>>,
    mut triggers: mpsc::Receiver<()>,
    outgoing: mpsc::Sender<Envelope>,
) -> Result<()> {
    let mut poll = tokio::time::interval(Duration::from_secs(60));
    let mut sample = tokio::time::interval(Duration::from_secs(30));
    let mut operations = tokio::time::interval(Duration::from_secs(5));
    operations.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    sample.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut reported = apsp_protocol::AppliedRevisions::new();
    let mut cleanup_storage = StorageRetry::default();
    loop {
        tokio::select! {
            _ = operations.tick(), if !reconcilers.is_empty() => {
                let _gate = if let Some(retirement) = &runtime.retirement {
                    Some(retirement.gate.read().await)
                } else { None };
                if runtime.retirement.as_ref().is_some_and(|retirement| retirement.requested()) { continue; }
                let active_client = { client.borrow().clone() };
                if let Some(active_client) = active_client
                {
                    if let Err(error) = crate::runtime_operations::poll(&reconcilers, &runtime.state, &active_client, &outgoing).await {
                        tracing::debug!(%error, "runtime operations poll failed");
                    }
                    if runtime.capabilities.iter().any(|capability| capability == apsp_protocol::RUNTIME_VALIDATION_CAPABILITY)
                        && !runtime.retirement.as_ref().is_some_and(|retirement| retirement.requested())
                        && let Err(error) = crate::runtime_validations::poll(&reconcilers, &runtime.state, &active_client).await {
                        tracing::debug!(%error, "runtime validations poll failed");
                    }
                }
                continue;
            }
            trigger = triggers.recv() => {
                if trigger.is_none() { return Ok(()); }
            }
            _ = poll.tick() => {}
            _ = sample.tick() => {
                let _gate = if let Some(retirement) = &runtime.retirement {
                    Some(retirement.gate.read().await)
                } else { None };
                if runtime.retirement.as_ref().is_some_and(|retirement| retirement.requested()) { continue; }
                for (module, reconciler) in &reconcilers {
                    if let Err(error) = reconciler.sample_usage().await {
                        tracing::warn!(%module, %error, "usage sampling failed");
                    }
                }
                {
                    let mut state = runtime.state.lock().map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
                    cleanup_storage.finish("clean acknowledged state", state.cleanup_acknowledged())?;
                }
                continue;
            }
        }
        let _gate = if let Some(retirement) = &runtime.retirement {
            Some(retirement.gate.read().await)
        } else {
            None
        };
        if runtime
            .retirement
            .as_ref()
            .is_some_and(|retirement| retirement.requested())
        {
            continue;
        }
        let active_client = { client.borrow().clone() };
        let Some(active_client) = active_client else {
            continue;
        };
        for (_, reconciler) in &reconcilers {
            if let Some(result) = reconciler.recovery_failure_result()? {
                let _ = outgoing.try_send(Envelope::new("apply.result", result)?);
            }
        }
        let manifest = match active_client.manifest().await {
            Ok(manifest) => manifest,
            Err(error) => {
                tracing::warn!(%error, "manifest fetch failed");
                continue;
            }
        };
        for (module, reconciler) in &reconcilers {
            let Some(target) = manifest.modules.get(module) else {
                continue;
            };
            let result = match reconciler.reconcile(target, &active_client).await {
                Ok(result) => result,
                Err(error) => ApplyResult {
                    module: module.clone(),
                    rev: target.config_rev,
                    op_id: Uuid::new_v4(),
                    status: ApplyStatus::Failed,
                    healthy: false,
                    error: Some(error.to_string()),
                },
            };
            // Results can be reconstructed on the next reconciliation if disconnected.
            let applied = result.status == ApplyStatus::Applied;
            let _ = outgoing.try_send(Envelope::new("apply.result", result)?);
            if applied {
                let current = runtime.applied()?;
                if current != reported
                    && let Some(info) = runtime.static_info()?
                    && outgoing
                        .try_send(Envelope::new("telemetry.static", info)?)
                        .is_ok()
                {
                    reported = current;
                }
            }
        }
    }
}

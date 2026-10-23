use crate::{SharedState, artifacts::PanelClient, reconcile::Reconciler};
use anyhow::{Result, ensure};
use apsp_protocol::{
    ApplyStatus, RuntimeOperation, RuntimeOperationError, RuntimeOperationRequest,
    RuntimeOperationResult, TaskAck, now_timestamp,
};
use serde::{Deserialize, Serialize};

const LEDGER_KEY: &str = "runtime_operations_v1";
const MAX_RECORDS: usize = 64;

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    request: RuntimeOperationRequest,
    result: Option<RuntimeOperationResult>,
    acknowledged: bool,
}

fn records(state: &SharedState) -> Result<Vec<Record>> {
    Ok(state
        .lock()
        .map_err(|_| anyhow::anyhow!("state poisoned"))?
        .get_json(LEDGER_KEY)?
        .unwrap_or_default())
}

fn save(state: &SharedState, records: &[Record]) -> Result<()> {
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("state poisoned"))?
        .set_json(LEDGER_KEY, &records)
}

fn result(
    request: &RuntimeOperationRequest,
    error: Option<RuntimeOperationError>,
    offset: i64,
) -> RuntimeOperationResult {
    RuntimeOperationResult {
        id: request.id,
        module: request.module.clone(),
        operation: request.operation,
        finished_at: now_timestamp()
            .saturating_add(offset)
            .max(request.requested_at),
        error,
        snapshot: None,
    }
}

async fn deliver(
    state: &SharedState,
    client: &PanelClient,
    ledger: &mut [Record],
    index: usize,
    offset: i64,
) -> Result<()> {
    // A durable request without a result belongs to an interrupted process.
    // Deployment intent recovery owns rollback; never repeat the operator action.
    if ledger[index].result.is_none() {
        ledger[index].result = Some(result(
            &ledger[index].request,
            Some(RuntimeOperationError::Interrupted),
            offset,
        ));
        save(state, ledger)?;
    }
    let completed = ledger[index].result.as_ref().expect("completed operation");
    let ack: TaskAck = client
        .post_json(
            &format!("/api/agent/v1/runtime-operations/{}", completed.id),
            completed,
        )
        .await?;
    ensure!(
        ack.ids == [completed.id],
        "runtime operation acknowledgment mismatch"
    );
    ledger[index].acknowledged = true;
    save(state, ledger)
}

/// The caller serializes this worker with reconciliation and holds the retirement gate.
pub(crate) async fn poll(
    reconcilers: &[(String, std::sync::Arc<Reconciler>)],
    state: &SharedState,
    client: &PanelClient,
    outgoing: &tokio::sync::mpsc::Sender<apsp_protocol::Envelope>,
) -> Result<()> {
    let offset = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state poisoned"))?
        .get_json::<i64>("clock_offset_ms")?
        .unwrap_or(0)
        / 1000;
    let mut ledger = records(state)?;
    ledger.retain(|record| {
        !record.acknowledged
            || record.request.expires_at.saturating_add(60)
                >= now_timestamp().saturating_add(offset)
    });
    // The panel may have committed a result whose response was lost. Flush the
    // durable outbox even when that request no longer appears in the pending set.
    for index in 0..ledger.len() {
        if !ledger[index].acknowledged {
            deliver(state, client, &mut ledger, index, offset).await?;
        }
    }
    let requests: Vec<RuntimeOperationRequest> =
        client.get_json("/api/agent/v1/runtime-operations").await?;
    ensure!(requests.len() <= 16, "too many runtime operations");
    for request in requests {
        ensure!(
            request.valid()
                && request.requested_at
                    <= now_timestamp().saturating_add(offset).saturating_add(60),
            "invalid runtime operation"
        );
        let index = if let Some(index) = ledger
            .iter()
            .position(|record| record.request.id == request.id)
        {
            ensure!(
                ledger[index].request == request,
                "runtime operation identity changed"
            );
            index
        } else {
            ensure!(ledger.len() < MAX_RECORDS, "runtime operation ledger full");
            ledger.push(Record {
                request: request.clone(),
                result: None,
                acknowledged: false,
            });
            save(state, &ledger)?;
            let index = ledger.len() - 1;
            let completed = execute(&request, reconcilers, client, outgoing, offset).await;
            ledger[index].result = Some(completed);
            save(state, &ledger)?;
            index
        };
        deliver(state, client, &mut ledger, index, offset).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

async fn execute(
    request: &RuntimeOperationRequest,
    reconcilers: &[(String, std::sync::Arc<Reconciler>)],
    client: &PanelClient,
    outgoing: &tokio::sync::mpsc::Sender<apsp_protocol::Envelope>,
    offset: i64,
) -> RuntimeOperationResult {
    if request.expires_at <= now_timestamp().saturating_add(offset) {
        return result(request, Some(RuntimeOperationError::Expired), offset);
    }
    let Some((_, reconciler)) = reconcilers
        .iter()
        .find(|(module, _)| module == &request.module)
    else {
        return result(
            request,
            Some(RuntimeOperationError::ModuleUnavailable),
            offset,
        );
    };
    let mut completed = result(request, None, offset);
    if request.operation != RuntimeOperation::Inspect {
        let manifest = match client.manifest().await {
            Ok(manifest) => manifest,
            Err(_) => {
                return result(
                    request,
                    Some(RuntimeOperationError::ManifestUnavailable),
                    offset,
                );
            }
        };
        let Some(target) = manifest.modules.get(&request.module) else {
            return result(
                request,
                Some(RuntimeOperationError::ModuleUnavailable),
                offset,
            );
        };
        if Some(target.config_rev) != request.expected_revision {
            return result(request, Some(RuntimeOperationError::TargetChanged), offset);
        }
        let success = match request.operation {
            RuntimeOperation::Restart => reconciler.restart_current(target, client).await.is_ok(),
            RuntimeOperation::RetryDeployment => match reconciler.reconcile(target, client).await {
                Ok(applied) => {
                    let success = applied.status == ApplyStatus::Applied;
                    if let Ok(envelope) = apsp_protocol::Envelope::new("apply.result", applied) {
                        let _ = outgoing.try_send(envelope);
                    }
                    success
                }
                Err(_) => false,
            },
            RuntimeOperation::Inspect => unreachable!(),
        };
        if !success {
            completed.error = Some(RuntimeOperationError::OperationFailed);
        }
    }
    match reconciler.runtime_snapshot().await {
        Ok(mut snapshot) => {
            snapshot.observed_at = snapshot
                .observed_at
                .saturating_add(offset)
                .max(request.requested_at);
            for entry in &mut snapshot.logs {
                entry.timestamp = entry
                    .timestamp
                    .map(|at| at.saturating_add(offset))
                    .filter(|at| *at > 0);
            }
            completed.snapshot = Some(snapshot);
        }
        Err(_) => completed.error = Some(RuntimeOperationError::OperationFailed),
    }
    if request.operation == RuntimeOperation::Restart
        && let Some(snapshot) = &completed.snapshot
    {
        let applied = apsp_protocol::ApplyResult {
            module: request.module.clone(),
            rev: request.expected_revision.unwrap_or_default(),
            op_id: request.id,
            status: if completed.error.is_none() {
                ApplyStatus::Applied
            } else {
                ApplyStatus::Failed
            },
            healthy: snapshot.healthy.unwrap_or(false),
            error: completed
                .error
                .map(|_| "runtime restart failed; inspect local service state".into()),
        };
        if let Ok(envelope) = apsp_protocol::Envelope::new("apply.result", applied) {
            let _ = outgoing.try_send(envelope);
        }
    }
    completed.finished_at = now_timestamp()
        .saturating_add(offset)
        .max(request.requested_at)
        .max(
            completed
                .snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.observed_at),
        );
    completed
}

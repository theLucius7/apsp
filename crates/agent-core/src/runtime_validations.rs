use crate::{SharedState, artifacts::PanelClient, reconcile::Reconciler};
use anyhow::{Result, ensure};
use apsp_protocol::{
    RuntimeValidationError, RuntimeValidationRequest, RuntimeValidationResult, TaskAck,
    now_timestamp,
};
use serde::{Deserialize, Serialize};

const KEY: &str = "runtime_validations_v1";

#[cfg(test)]
mod tests;

#[derive(Serialize, Deserialize)]
struct Record {
    request: RuntimeValidationRequest,
    result: Option<RuntimeValidationResult>,
    acknowledged: bool,
}

fn save(state: &SharedState, records: &[Record]) -> Result<()> {
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("state poisoned"))?
        .set_json(KEY, &records)
}

async fn deliver(client: &PanelClient, result: &RuntimeValidationResult) -> Result<()> {
    let ack: TaskAck = client
        .post_json(
            &format!(
                "/api/agent/v1/runtime-validations/{}/result",
                result.request.id
            ),
            result,
        )
        .await?;
    ensure!(
        ack.ids == [result.request.id],
        "runtime validation acknowledgment mismatch"
    );
    Ok(())
}

/// Shares the reconciliation worker and retirement gate; each turn executes at most one probe.
pub(crate) async fn poll(
    reconcilers: &[(String, std::sync::Arc<Reconciler>)],
    state: &SharedState,
    client: &PanelClient,
) -> Result<()> {
    let (mut records, offset): (Vec<Record>, i64) = {
        let state = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?;
        (
            state.get_json(KEY)?.unwrap_or_default(),
            state
                .get_json::<i64>("clock_offset_ms")?
                .unwrap_or_default()
                / 1000,
        )
    };
    let now = || now_timestamp().saturating_add(offset);
    records.retain(|record| {
        !record.acknowledged || record.request.expires_at.saturating_add(60) >= now()
    });
    for index in 0..records.len() {
        if records[index].acknowledged {
            continue;
        }
        if records[index].result.is_none() {
            records[index].result = Some(RuntimeValidationResult {
                request: records[index].request.clone(),
                success: false,
                error: Some(RuntimeValidationError::Interrupted),
                checked_at: now(),
            });
            save(state, &records)?;
        }
        deliver(
            client,
            records[index]
                .result
                .as_ref()
                .expect("completed validation"),
        )
        .await?;
        records[index].acknowledged = true;
        save(state, &records)?;
    }
    let requests: Vec<RuntimeValidationRequest> =
        client.get_json("/api/agent/v1/runtime-validations").await?;
    ensure!(requests.len() <= 16, "too many runtime validations");
    let Some(request) = requests.into_iter().next() else {
        return Ok(());
    };
    ensure!(
        request.valid() && request.expires_at.saturating_sub(now()) <= 3600,
        "invalid runtime validation"
    );
    if let Some(record) = records
        .iter()
        .find(|record| record.request.id == request.id)
    {
        ensure!(
            record.request == request,
            "runtime validation identity changed"
        );
        return deliver(
            client,
            record.result.as_ref().expect("completed validation"),
        )
        .await;
    }
    ensure!(records.len() < 128, "runtime validation ledger full");
    records.push(Record {
        request: request.clone(),
        result: None,
        acknowledged: false,
    });
    save(state, &records)?;
    let error = if request.expires_at <= now() {
        Some(RuntimeValidationError::Expired)
    } else if let Some((_, reconciler)) = reconcilers
        .iter()
        .find(|(module, _)| module == &request.module)
    {
        reconciler.validate_runtime_dependency(&request).await.err()
    } else {
        Some(RuntimeValidationError::ModuleUnavailable)
    };
    let mut result = RuntimeValidationResult {
        request,
        success: error.is_none(),
        error,
        checked_at: now(),
    };
    if result.checked_at >= result.request.expires_at {
        result.success = false;
        result.error = Some(RuntimeValidationError::Expired);
    }
    let index = records.len() - 1;
    records[index].result = Some(result.clone());
    save(state, &records)?;
    deliver(client, &result).await?;
    records[index].acknowledged = true;
    save(state, &records)
}

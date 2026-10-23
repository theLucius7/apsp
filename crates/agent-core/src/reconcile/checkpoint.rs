use super::Reconciler;
use crate::state::runtime_control::{ControlRequest, ControlResult};
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::{Prepared, RuntimeInstance};
use apsp_protocol::{
    ApplyResult, ApplyStatus, RuntimeBinding, RuntimeCheckpoint, RuntimeCheckpointResult,
    RuntimeRecoveryBarrierResult,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Activation {
    activation_id: Uuid,
    revision: u64,
    pub(super) bundle_sha256: String,
    instance: RuntimeInstance,
}

impl Reconciler {
    pub(super) fn revision_floor(&self) -> Result<u64> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?
            .runtime_revision_floor(&self.adapter.describe().module)
    }

    pub(super) fn require_revision_floor(&self, revision: u64) -> Result<()> {
        ensure!(
            revision >= self.revision_floor()?,
            "deployment is below the committed recovery revision floor"
        );
        Ok(())
    }

    pub(super) fn saved_activation(&self) -> Result<Option<Activation>> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?
            .get_json::<Option<Activation>>(&format!(
                "runtime_activation:{}",
                self.adapter.describe().module
            ))
            .map(Option::flatten)
    }

    pub(super) async fn validate_activation(
        &self,
        runtime: &Prepared,
        activation: &Activation,
    ) -> Result<()> {
        let result: Result<()> = async {
            ensure!(
                !activation.activation_id.is_nil()
                    && activation.revision == runtime.spec.revision
                    && activation.bundle_sha256 == runtime.spec.config_hash,
                "certified activation does not identify the applied revision"
            );
            ensure!(
                self.inspect_runtime(runtime).await? == activation.instance,
                "controlled runtime instance differs from the successfully applied activation"
            );
            Ok(())
        }
        .await;
        if result.is_err() {
            self.state
                .lock()
                .map_err(|_| anyhow::anyhow!("state poisoned"))?
                .set_json(
                    &format!("health:{}", self.adapter.describe().module),
                    &false,
                )?;
        }
        result
    }

    pub(super) async fn activation_after_apply(
        &self,
        runtime: &Prepared,
        preserved: Option<&Activation>,
    ) -> Result<Option<Activation>> {
        if !self.services.supports_runtime_checkpoint() {
            return Ok(None);
        }
        let instance = self.inspect_runtime(runtime).await?;
        if let Some(preserved) = preserved {
            ensure!(
                preserved.bundle_sha256 == runtime.spec.config_hash
                    && preserved.instance == instance,
                "unchanged deployment cannot certify a different configuration or process instance"
            );
        }
        Ok(Some(Activation {
            activation_id: preserved
                .map(|old| old.activation_id)
                .unwrap_or_else(Uuid::new_v4),
            revision: runtime.spec.revision,
            bundle_sha256: runtime.spec.config_hash.clone(),
            instance,
        }))
    }

    pub(super) async fn observe_checkpoint(
        &self,
        binding: &RuntimeBinding,
    ) -> Result<RuntimeCheckpoint> {
        ensure!(
            binding.valid() && binding.module == self.adapter.describe().module,
            "checkpoint module or binding is invalid"
        );
        ensure!(
            self.services.supports_runtime_checkpoint(),
            "exact runtime inspection is unsupported"
        );
        let current = self.previous()?.context("no applied runtime exists")?;
        ensure!(
            current.spec.revision == binding.revision
                && current.spec.config_hash == binding.bundle_sha256,
            "expected deployment differs from the applied revision or bundle"
        );
        self.require_revision_floor(current.spec.revision)?;
        let key = format!("runtime_binding:{}:{}", binding.module, binding.revision);
        {
            let state = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("state poisoned"))?;
            if let Some(saved) = state.get_json::<RuntimeBinding>(&key)? {
                ensure!(
                    saved == *binding,
                    "this revision is already associated with another deployment binding"
                );
            }
        }
        let activation = self.saved_activation()?.context(
            "runtime has no certified activation; administrator must create a new deployment before certification")?;
        self.validate_activation(&current, &activation).await?;
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?
            .set_json_batch(&[
                (key, serde_json::to_value(binding)?),
                (
                    format!("health:{}", binding.module),
                    serde_json::json!(true),
                ),
            ])?;
        Ok(RuntimeCheckpoint {
            binding: binding.clone(),
            activation_id: activation.activation_id,
            instance_id: activation.instance.instance_id,
            healthy: true,
        })
    }

    pub(super) fn runtime_control_now(&self) -> Result<i64> {
        let offset = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?
            .get_json::<i64>("clock_offset_ms")?
            .unwrap_or(0);
        Ok(apsp_protocol::telemetry::now_millis()
            .saturating_add(offset)
            .div_euclid(1000))
    }

    pub(crate) fn recovery_failure_result(&self) -> Result<Option<ApplyResult>> {
        #[derive(Deserialize)]
        struct Failure {
            revision: u64,
            error: String,
        }
        let module = self.adapter.describe().module;
        let failure = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?
            .get_json::<Failure>(&format!("runtime_recovery_error:{module}"))?;
        failure
            .map(|failure| {
                self.stable_apply_result(ApplyResult {
                    module,
                    rev: failure.revision,
                    op_id: Uuid::new_v4(),
                    status: ApplyStatus::Failed,
                    healthy: false,
                    error: Some(failure.error),
                })
            })
            .transpose()
    }

    /// The same gate protects apply, recovery, observation and the atomic barrier receipt.
    pub async fn runtime_control(&self, request: &ControlRequest) -> Result<ControlResult> {
        let _guard = self.gate.lock().await;
        if let Some(saved) = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?
            .runtime_control_result(request)?
        {
            return Ok(saved);
        }
        let digest = request.digest()?;
        if let ControlRequest::Probe(probe) = request {
            return self.execute_path_probe(probe, digest).await;
        }
        let attempt: Result<(RuntimeCheckpoint, Option<u64>)> = async {
            ensure!(request.module() == self.adapter.describe().module, "runtime control module mismatch");
            ensure!(request.valid_at(self.runtime_control_now()?), "runtime control request expired or exceeds the allowed deadline");
            let binding = match request {
                ControlRequest::Checkpoint(value) => &value.expected,
                ControlRequest::Barrier(value) => &value.expected.binding,
                ControlRequest::Probe(_) => unreachable!("probe handled above"),
            };
            self.require_revision_floor(binding.revision)?;
            ensure!(self.services.supports_runtime_checkpoint(), "exact runtime inspection is unsupported");
            let clear = self.state.lock().map_err(|_| anyhow::anyhow!("state poisoned"))?
                .pending_intents()?.iter().all(|intent| intent.module != binding.module);
            ensure!(clear, "unfinished runtime intent must be recovered by managed reconciliation before certification");
            let observed = self.observe_checkpoint(binding).await?;
            let floor = match request {
                ControlRequest::Checkpoint(_) => None,
                ControlRequest::Probe(_) => unreachable!("probe handled above"),
                ControlRequest::Barrier(value) => {
                    ensure!(observed == value.expected, "expected activation or runtime instance differs from the observed checkpoint");
                    ensure!(value.minimum_revision >= self.revision_floor()? && value.minimum_revision <= observed.binding.revision,
                        "requested recovery floor decreases a promise or exceeds the current revision");
                    let clear = self.state.lock().map_err(|_| anyhow::anyhow!("state poisoned"))?
                        .pending_intents()?.iter().all(|intent| intent.module != binding.module);
                    ensure!(clear, "unfinished runtime intent prevents a recovery barrier");
                    Some(value.minimum_revision)
                }
            };
            ensure!(request.valid_at(self.runtime_control_now()?), "runtime control request expired during inspection");
            Ok((observed, floor))
        }.await;
        let (observed, floor, error) = match attempt {
            Ok((observed, floor)) => (Some(observed), floor, None),
            Err(error) => (None, None, Some(bounded_error(&error))),
        };
        let result = match request {
            ControlRequest::Checkpoint(value) => {
                ControlResult::Checkpoint(RuntimeCheckpointResult {
                    request_id: value.request_id,
                    request_digest: digest,
                    observed,
                    success: error.is_none(),
                    error,
                })
            }
            ControlRequest::Probe(_) => unreachable!("probe handled above"),
            ControlRequest::Barrier(value) => {
                ControlResult::Barrier(RuntimeRecoveryBarrierResult {
                    request_id: value.request_id,
                    request_digest: digest,
                    observed,
                    minimum_revision: floor,
                    pending_intents_clear: error.is_none(),
                    success: error.is_none(),
                    error,
                })
            }
        };
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?
            .finish_runtime_control(&result, floor.map(|revision| (request.module(), revision)))?;
        Ok(result)
    }
}

pub(crate) fn bounded_error(error: &anyhow::Error) -> String {
    let message: String = format!("{error:#}")
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_control() {
                c
            } else {
                ' '
            }
        })
        .take(1024)
        .collect();
    if message.trim().is_empty() {
        "runtime control failed".into()
    } else {
        message
    }
}

#[cfg(all(test, unix))]
mod tests;

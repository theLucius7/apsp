use super::Reconciler;
use anyhow::{Result, ensure};
use apsp_adapter_sdk::Prepared;
use apsp_protocol::{
    RuntimeValidationError, RuntimeValidationOperation, RuntimeValidationRequest,
    valid_runtime_scope,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Constraints {
    pub schema: u32,
    pub active: BTreeMap<String, u64>,
    pub retired: BTreeMap<String, u64>,
}

impl Constraints {
    fn parse(runtime: &Prepared) -> Result<Self> {
        let Some(text) = runtime.spec.files.get("runtime-constraints.json") else {
            return Ok(Self {
                schema: 1,
                ..Self::default()
            });
        };
        ensure!(
            text.len() <= 64 * 1024,
            "runtime constraints exceed size limit"
        );
        let constraints: Self = serde_json::from_str(text)?;
        ensure!(
            constraints.schema == 1 && constraints.active.len() + constraints.retired.len() <= 1024,
            "invalid runtime constraints schema or count"
        );
        for (scope, generation) in constraints.active.iter().chain(&constraints.retired) {
            ensure!(
                valid_runtime_scope(scope) && *generation > 0 && *generation <= i64::MAX as u64,
                "invalid runtime constraint"
            );
        }
        ensure!(
            constraints
                .active
                .keys()
                .all(|scope| !constraints.retired.contains_key(scope)),
            "ambiguous runtime constraint"
        );
        Ok(constraints)
    }

    fn covers(&self, floors: &BTreeMap<String, u64>) -> bool {
        floors.iter().all(|(scope, floor)| {
            self.active
                .get(scope)
                .or_else(|| self.retired.get(scope))
                .is_some_and(|generation| generation >= floor)
        })
    }
}

impl Reconciler {
    fn floor_key(&self) -> String {
        format!(
            "runtime_generation_floor:{}",
            self.adapter.describe().module
        )
    }

    pub(super) fn enforce_generation_floor(&self, runtime: &Prepared) -> Result<()> {
        let constraints = Constraints::parse(runtime)?;
        let floors: BTreeMap<String, u64> = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?
            .get_json(&self.floor_key())?
            .unwrap_or_default();
        ensure!(
            constraints.covers(&floors),
            "configuration is below the committed recovery generation"
        );
        Ok(())
    }

    pub async fn validate_runtime_dependency(
        &self,
        request: &RuntimeValidationRequest,
    ) -> Result<(), RuntimeValidationError> {
        let _guard = self.gate.lock().await;
        if !request.valid() || request.module != self.adapter.describe().module {
            return Err(RuntimeValidationError::ModuleUnavailable);
        }
        if self
            .state
            .lock()
            .map_err(|_| RuntimeValidationError::ValidationFailed)?
            .pending_intents()
            .map_err(|_| RuntimeValidationError::ValidationFailed)?
            .iter()
            .any(|intent| intent.module == request.module)
        {
            return Err(RuntimeValidationError::ValidationFailed);
        }
        let runtime = self
            .previous()
            .map_err(|_| RuntimeValidationError::ValidationFailed)?
            .ok_or(RuntimeValidationError::RevisionChanged)?;
        if runtime.spec.revision != request.revision
            || runtime.spec.config_hash != request.config_hash
        {
            return Err(RuntimeValidationError::RevisionChanged);
        }
        self.verify_applied_runtime(&runtime)
            .await
            .map_err(|_| RuntimeValidationError::UntrustedArtifact)?;
        let offset = self
            .state
            .lock()
            .map_err(|_| RuntimeValidationError::ValidationFailed)?
            .get_json::<i64>("clock_offset_ms")
            .map_err(|_| RuntimeValidationError::ValidationFailed)?
            .unwrap_or(0)
            / 1000;
        let now = || apsp_protocol::now_timestamp().saturating_add(offset);
        if request.expires_at <= now() {
            return Err(RuntimeValidationError::Expired);
        }
        match request.operation {
            RuntimeValidationOperation::Probe => {
                if !self.adapter.supports_dependency_validation() {
                    return Err(RuntimeValidationError::ModuleUnavailable);
                }
                let budget = request.expires_at.saturating_sub(now()).clamp(1, 15) as u64;
                tokio::time::timeout(
                    Duration::from_secs(budget),
                    self.adapter
                        .validate_dependency(&runtime, &request.scope, request.generation),
                )
                .await
                .map_err(|_| RuntimeValidationError::ValidationFailed)?
                .map_err(|_| RuntimeValidationError::ValidationFailed)?;
            }
            RuntimeValidationOperation::Barrier => {
                let constraints = Constraints::parse(&runtime)
                    .map_err(|_| RuntimeValidationError::InvalidConstraints)?;
                if !constraints
                    .active
                    .get(&request.scope)
                    .is_some_and(|generation| *generation >= request.generation)
                {
                    return Err(RuntimeValidationError::InvalidConstraints);
                }
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| RuntimeValidationError::ValidationFailed)?;
                let mut floors: BTreeMap<String, u64> = state
                    .get_json(&self.floor_key())
                    .map_err(|_| RuntimeValidationError::ValidationFailed)?
                    .unwrap_or_default();
                floors
                    .entry(request.scope.clone())
                    .and_modify(|generation| *generation = (*generation).max(request.generation))
                    .or_insert(request.generation);
                state
                    .set_json(&self.floor_key(), &floors)
                    .map_err(|_| RuntimeValidationError::ValidationFailed)?;
            }
        }
        if request.expires_at <= now() {
            return Err(RuntimeValidationError::Expired);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn floor_requires_explicit_active_or_retired_generation() {
        let floors = BTreeMap::from([("scope:1".into(), 3)]);
        let mut constraints = Constraints {
            schema: 1,
            ..Default::default()
        };
        assert!(!constraints.covers(&floors));
        constraints.active.insert("scope:1".into(), 2);
        assert!(!constraints.covers(&floors));
        constraints.active.insert("scope:1".into(), 3);
        assert!(constraints.covers(&floors));
        constraints.active.clear();
        constraints.retired.insert("scope:1".into(), 3);
        assert!(constraints.covers(&floors));
    }
}

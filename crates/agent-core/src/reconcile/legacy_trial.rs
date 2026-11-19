//! A released 0.3.x supervisor only rolls back the Agent executable. It does,
//! however, run a worker which recovers the existing ApplyIntent journal.
use super::*;
use crate::state::IntentRecord;

const GUARD: &str = "agent_upgrade_trial";

impl Reconciler {
    pub(crate) fn legacy_trial(&self) -> Result<Option<String>> {
        Ok(crate::upgrade::installed_trial(&self.config.agent_root)?
            .filter(|(state, _)| state.trial_snapshot.is_none())
            .map(|(_, version)| version))
    }

    // A 0.3.x worker cannot undo the new desired key. Keep it speculative in
    // the trial and commit it only when the supervisor selects that release.
    pub(super) fn desired(&self, module: &str) -> Result<Option<bool>> {
        let mut db = self.state.locked()?;
        let key = format!("desired:{module}");
        let staging = format!("upgrade_desired:{module}");
        if let Some((version, value)) = db.get_json::<(String, bool)>(&staging)? {
            if self.legacy_trial()?.as_ref() == Some(&version) {
                return Ok(Some(value));
            }
            if crate::upgrade::state(&self.config)?.is_some_and(|s| s.current == version) {
                db.set_json(&key, &value)?;
            }
            db.remove_json(&staging)?;
        }
        db.get_json(&key)
    }

    pub(super) fn set_desired(&self, module: &str, value: bool) -> Result<()> {
        if let Some(version) = self.legacy_trial()? {
            return self
                .state
                .locked()?
                .set_json(&format!("upgrade_desired:{module}"), &(version, value));
        }
        self.state
            .locked()?
            .set_json(&format!("desired:{module}"), &value)
    }

    pub(crate) async fn finish_confirmed_trial(&self) -> Result<()> {
        let _guard = self.gate.lock().await;
        self.desired(&self.descriptor().module)?;
        self.recover_locked().await
    }

    /// Keep the original live configuration recoverable even after the new
    /// worker has checkpointed a successful apply but before core promotion.
    pub(super) fn guard_trial(&self, prior: Option<&Prepared>, target: &Prepared) -> Result<bool> {
        let Some(version) = self.legacy_trial()? else {
            return Ok(false);
        };
        let module = self.descriptor().module;
        let mut state = self.state.locked()?;
        if !state
            .pending_intents()?
            .iter()
            .any(|r| r.module == module && r.payload[GUARD].as_str() == Some(&version))
        {
            let intent = ApplyIntent {
                previous: prior.cloned(),
                target: target.clone(),
                plan: Plan::Restart,
            };
            let mut payload = serde_json::to_value(intent)?;
            payload[GUARD] = version.into();
            state.begin_intent(&IntentRecord {
                op_id: Uuid::new_v4(),
                module,
                payload,
            })?;
        }
        Ok(true)
    }

    pub(super) fn preserve_trial_intent(&self, record: &IntentRecord) -> Result<bool> {
        let Some(version) = record.payload[GUARD].as_str() else {
            return Ok(false);
        };
        if self.legacy_trial()?.as_deref() == Some(version) {
            return Ok(true);
        }
        if let Some(state) = crate::upgrade::state(&self.config)?
            && state.current == version
        {
            self.state.locked()?.finish_intent(record.op_id)?;
            return Ok(true);
        }
        Ok(false)
    }
}

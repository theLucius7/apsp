//! A released 0.3.x supervisor only rolls back the Agent executable. It does,
//! however, run a worker which recovers the existing ApplyIntent journal.
use super::*;
use crate::state::{IntentRecord, State, write_json};

const GUARD: &str = "agent_upgrade_trial";

fn commit_desired(db: &mut State, module: &str, value: bool) -> Result<()> {
    let transaction = db.connection.transaction()?;
    write_json(&transaction, &format!("desired:{module}"), &value)?;
    transaction.execute(
        "DELETE FROM kv WHERE key=?1",
        [format!("upgrade_desired:{module}")],
    )?;
    transaction.commit()?;
    Ok(())
}

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
                commit_desired(&mut db, module, value)?;
                return Ok(Some(value));
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
        let mut db = self.state.locked()?;
        commit_desired(&mut db, module, value)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fake::{FakeAdapter, FakeServiceManager},
        system::SystemOps,
    };
    use std::{fs, path::Path};

    #[test]
    fn post_promotion_desired_update_supersedes_staged_value() {
        let agent_root = std::env::temp_dir().join(format!("apsp-desired-{}", Uuid::new_v4()));
        fs::create_dir(&agent_root).unwrap();
        let upgrade = crate::upgrade::UpgradeState {
            current: "0.4.0".into(),
            ..Default::default()
        };
        fs::write(
            agent_root.join("update-state.json"),
            serde_json::to_vec(&upgrade).unwrap(),
        )
        .unwrap();
        let state = Arc::new(std::sync::Mutex::new(
            State::open(Path::new(":memory:")).unwrap(),
        ));
        state
            .locked()
            .unwrap()
            .set_json("upgrade_desired:demo", &("0.4.0", true))
            .unwrap();
        let reconciler = Reconciler::new(
            Config {
                agent_root: agent_root.clone(),
                ..Config::default()
            },
            state.clone(),
            Arc::new(FakeAdapter::default()),
            Arc::new(SystemOps),
            Arc::new(FakeServiceManager::default()),
        );
        reconciler.set_desired("demo", false).unwrap();
        assert_eq!(
            state
                .locked()
                .unwrap()
                .get_json::<(String, bool)>("upgrade_desired:demo")
                .unwrap(),
            None
        );
        assert_eq!(reconciler.desired("demo").unwrap(), Some(false));
        fs::remove_dir_all(agent_root).unwrap();
    }
}

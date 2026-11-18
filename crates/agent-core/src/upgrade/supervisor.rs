use super::*;
use crate::system::deploy::reference;
use apsp_adapter_sdk::ManagedProcess;

async fn save(root: &Path, state: &UpgradeState, ops: &dyn Privileged) -> Result<()> {
    ops.write_file(
        &root.join("update-state.json"),
        &serde_json::to_vec_pretty(state)?,
        0o600,
        None,
    )
    .await
}
async fn spawn(
    root: &Path,
    version: &str,
    path: &Path,
    monitor_only: bool,
    ops: &dyn Privileged,
) -> Result<Box<dyn ManagedProcess>> {
    ensure!(
        release_version(version).is_some(),
        "invalid installed Agent version"
    );
    let mut args = vec![
        "--config".into(),
        path.to_string_lossy().into_owned(),
        "run".into(),
    ];
    if monitor_only {
        args.push("--monitor-only".into());
    }
    crate::artifacts::verify_expected_agent(
        &root.join(version).join(executable_name()),
        version,
        &apsp_protocol::release::TrustedKeys::compiled()?,
    )
    .await?;
    ops.spawn_managed(&root.join(version).join(executable_name()), &args)
        .await
}

fn has_exited(process: &mut dyn ManagedProcess) -> Result<bool> {
    let exited = process.try_wait()?.is_some();
    if exited && process.exit_code() == Some(crate::retirement::RETIRED_EXIT_CODE) {
        return Err(crate::retirement::Retired.into());
    }
    Ok(exited)
}

fn capture_install_links(
    install_root: &Path,
) -> Result<(std::collections::BTreeMap<String, String>, BTreeSet<String>)> {
    let mut installs = std::collections::BTreeMap::new();
    let mut preexisting_links = BTreeSet::new();
    let entries = match std::fs::read_dir(install_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((installs, preexisting_links));
        }
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        let Ok(artifact_name) = entry.file_name().into_string() else {
            continue;
        };
        if !crate::artifacts::safe_component(&artifact_name) {
            continue;
        }
        let artifact_dir = entry.path();
        let current_link = artifact_dir.join("current");
        if std::fs::symlink_metadata(&current_link).is_err() {
            continue;
        }
        preexisting_links.insert(artifact_name.clone());
        let Ok(target) = reference(&current_link) else {
            continue;
        };
        let Ok(resolved) = std::fs::canonicalize(&target) else {
            continue;
        };
        let Ok(canonical_dir) = std::fs::canonicalize(&artifact_dir) else {
            continue;
        };
        if resolved.parent() != Some(canonical_dir.as_path()) {
            continue;
        }
        if let Some(version_dir) = resolved.file_name().and_then(|name| name.to_str())
            && apsp_protocol::bundled::valid_version_segment(version_dir)
        {
            installs.insert(artifact_name, version_dir.to_owned());
        }
    }
    Ok((installs, preexisting_links))
}

fn capture_runtime_links(
    runtime_root: &Path,
) -> Result<(std::collections::BTreeMap<String, String>, BTreeSet<String>)> {
    let mut runtimes = std::collections::BTreeMap::new();
    let mut preexisting_links = BTreeSet::new();
    let entries = match std::fs::read_dir(runtime_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((runtimes, preexisting_links));
        }
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        let Ok(unit_dir_name) = entry.file_name().into_string() else {
            continue;
        };
        let Some(artifact_name) = unit_dir_name.strip_suffix("@main") else {
            continue;
        };
        if !crate::artifacts::safe_component(artifact_name) {
            continue;
        }
        let unit_dir = entry.path();
        let current_link = unit_dir.join("current");
        if std::fs::symlink_metadata(&current_link).is_err() {
            continue;
        }
        preexisting_links.insert(unit_dir_name.clone());
        let Ok(target) = reference(&current_link) else {
            continue;
        };
        let Ok(resolved) = std::fs::canonicalize(&target) else {
            continue;
        };
        let Ok(canonical_revisions) = std::fs::canonicalize(unit_dir.join("revisions")) else {
            continue;
        };
        if resolved.parent() != Some(canonical_revisions.as_path()) {
            continue;
        }
        if let Some(rev_dir) = resolved.file_name().and_then(|name| name.to_str())
            && crate::artifacts::safe_component(rev_dir)
        {
            runtimes.insert(unit_dir_name, rev_dir.to_owned());
        }
    }
    Ok((runtimes, preexisting_links))
}

fn capture_trial_snapshot(config: &Config) -> Result<TrialSnapshot> {
    let (install_currents, preexisting_install_links) =
        capture_install_links(&config.install_root)?;
    let (runtime_currents, preexisting_runtime_links) =
        capture_runtime_links(&config.runtime_root)?;
    let mut snapshot = TrialSnapshot {
        install_currents,
        preexisting_install_links,
        runtime_currents,
        preexisting_runtime_links,
        ..TrialSnapshot::default()
    };
    match std::fs::symlink_metadata(&config.state_db) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(snapshot),
        Err(error) => return Err(error.into()),
        Ok(meta) => ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "state database must be an ordinary file"
        ),
    }
    snapshot.db_existed = true;
    let connection = rusqlite::Connection::open_with_flags(
        &config.state_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let mut modules = BTreeSet::new();
    {
        let mut stmt = connection.prepare(
            "SELECT key, value FROM kv WHERE key LIKE 'applied:%' OR key LIKE 'desired:%' OR key LIKE 'health:%' OR key LIKE 'runtime_activation:%' OR key LIKE 'apply_receipt:%'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (key, raw) = row?;
            if let Some(module) = key.strip_prefix("applied:") {
                modules.insert(module.to_owned());
                let parsed: Option<apsp_adapter_sdk::Prepared> = serde_json::from_str(&raw)?;
                snapshot.applied.insert(module.to_owned(), parsed);
            } else if let Some(module) = key.strip_prefix("desired:") {
                modules.insert(module.to_owned());
                let parsed: Option<bool> = serde_json::from_str(&raw)?;
                snapshot.desired.insert(module.to_owned(), parsed);
            } else if let Some(module) = key.strip_prefix("health:") {
                modules.insert(module.to_owned());
                let parsed: Option<bool> = serde_json::from_str(&raw)?;
                snapshot.health.insert(module.to_owned(), parsed);
            } else if let Some(module) = key.strip_prefix("runtime_activation:") {
                modules.insert(module.to_owned());
                let parsed: serde_json::Value = serde_json::from_str(&raw)?;
                snapshot
                    .activations
                    .insert(module.to_owned(), (!parsed.is_null()).then_some(parsed));
            } else if let Some(module) = key.strip_prefix("apply_receipt:") {
                modules.insert(module.to_owned());
                let parsed: serde_json::Value = serde_json::from_str(&raw)?;
                snapshot
                    .receipts
                    .insert(module.to_owned(), (!parsed.is_null()).then_some(parsed));
            }
        }
    }
    for module in modules {
        snapshot.applied.entry(module.clone()).or_insert(None);
        snapshot.desired.entry(module.clone()).or_insert(None);
        snapshot.health.entry(module.clone()).or_insert(None);
        snapshot.activations.entry(module.clone()).or_insert(None);
        snapshot.receipts.entry(module).or_insert(None);
    }
    {
        let mut stmt = connection.prepare("SELECT op_id FROM intents WHERE completed = 0")?;
        for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
            snapshot.preexisting_intents.insert(row?);
        }
    }
    Ok(snapshot)
}

async fn restore_previous_installs(
    install_root: &Path,
    previous_installs: &std::collections::BTreeMap<String, String>,
    ops: &dyn Privileged,
) -> Result<()> {
    for (artifact_name, version_dir) in previous_installs {
        if !crate::artifacts::safe_component(artifact_name)
            || !apsp_protocol::bundled::valid_version_segment(version_dir)
        {
            continue;
        }
        let artifact_dir = install_root.join(artifact_name);
        let target = artifact_dir.join(version_dir);
        if std::fs::symlink_metadata(&target)
            .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
        {
            ops.atomic_symlink(&artifact_dir.join("current"), &target)
                .await?;
        }
    }
    Ok(())
}

async fn restore_trial_state(
    config: &Config,
    state: &mut UpgradeState,
    ops: &dyn Privileged,
) -> Result<()> {
    if let Some(snapshot) = state.trial_snapshot.take() {
        restore_previous_installs(&config.install_root, &snapshot.install_currents, ops).await?;
        if let Ok(entries) = std::fs::read_dir(&config.install_root) {
            for entry in entries {
                let entry = entry?;
                let file_type = entry.file_type()?;
                if file_type.is_symlink() || !file_type.is_dir() {
                    continue;
                }
                let Ok(artifact_name) = entry.file_name().into_string() else {
                    continue;
                };
                if !crate::artifacts::safe_component(&artifact_name)
                    || snapshot.preexisting_install_links.contains(&artifact_name)
                {
                    continue;
                }
                let artifact_dir = entry.path();
                let current_link = artifact_dir.join("current");
                let Ok(target) = reference(&current_link) else {
                    continue;
                };
                let Ok(resolved) = std::fs::canonicalize(&target) else {
                    continue;
                };
                let Ok(canonical_dir) = std::fs::canonicalize(&artifact_dir) else {
                    continue;
                };
                if resolved.parent() == Some(canonical_dir.as_path())
                    && crate::bundled::is_bundled_directory_sync(&resolved)
                {
                    ops.remove_symlink(&current_link).await?;
                }
            }
        }

        for (unit_dir_name, rev_dir) in &snapshot.runtime_currents {
            let Some(artifact_name) = unit_dir_name.strip_suffix("@main") else {
                continue;
            };
            if !crate::artifacts::safe_component(artifact_name)
                || !crate::artifacts::safe_component(rev_dir)
            {
                continue;
            }
            let unit_dir = config.runtime_root.join(unit_dir_name);
            let target = unit_dir.join("revisions").join(rev_dir);
            if std::fs::symlink_metadata(&target)
                .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
            {
                ops.atomic_symlink(&unit_dir.join("current"), &target)
                    .await?;
            }
        }
        if let Ok(entries) = std::fs::read_dir(&config.runtime_root) {
            for entry in entries {
                let entry = entry?;
                let file_type = entry.file_type()?;
                if file_type.is_symlink() || !file_type.is_dir() {
                    continue;
                }
                let Ok(unit_dir_name) = entry.file_name().into_string() else {
                    continue;
                };
                let Some(artifact_name) = unit_dir_name.strip_suffix("@main") else {
                    continue;
                };
                if !crate::artifacts::safe_component(artifact_name)
                    || snapshot.preexisting_runtime_links.contains(&unit_dir_name)
                {
                    continue;
                }
                let unit_dir = entry.path();
                let current_link = unit_dir.join("current");
                let Ok(target) = reference(&current_link) else {
                    continue;
                };
                let Ok(resolved) = std::fs::canonicalize(&target) else {
                    continue;
                };
                let Ok(canonical_revisions) = std::fs::canonicalize(unit_dir.join("revisions"))
                else {
                    continue;
                };
                if resolved.parent() == Some(canonical_revisions.as_path()) {
                    ops.remove_symlink(&current_link).await?;
                }
            }
        }

        if config.state_db.try_exists()? {
            let mut db = crate::State::open(&config.state_db)?;
            let mut modules: BTreeSet<String> = snapshot
                .applied
                .keys()
                .chain(snapshot.desired.keys())
                .chain(snapshot.health.keys())
                .chain(snapshot.activations.keys())
                .chain(snapshot.receipts.keys())
                .cloned()
                .collect();
            {
                let mut stmt = db.connection.prepare(
                    "SELECT key FROM kv WHERE key LIKE 'applied:%' OR key LIKE 'desired:%' OR key LIKE 'health:%' OR key LIKE 'runtime_activation:%' OR key LIKE 'apply_receipt:%' OR key LIKE 'artifact_error:%' OR key LIKE 'runtime_recovery_error:%'",
                )?;
                let keys: Vec<String> = stmt
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for key in keys {
                    if let Some((_, module)) = key.split_once(':') {
                        modules.insert(module.to_owned());
                    }
                }
            }
            let pending_intents = db.pending_intents()?;
            let mut trial_targets = std::collections::BTreeMap::new();
            for record in &pending_intents {
                if !snapshot
                    .preexisting_intents
                    .contains(&record.op_id.to_string())
                {
                    if let Ok(intent) = serde_json::from_value::<crate::reconcile::ApplyIntent>(
                        record.payload.clone(),
                    ) {
                        trial_targets.insert(record.module.clone(), intent.target);
                    }
                    db.finish_intent(record.op_id)?;
                }
            }
            for module in modules {
                let pre_applied = snapshot.applied.get(&module).and_then(Clone::clone);
                let post_applied =
                    db.get_json::<apsp_adapter_sdk::Prepared>(&format!("applied:{module}"))?;
                let pre_desired = snapshot.desired.get(&module).and_then(Clone::clone);
                let pre_health = snapshot.health.get(&module).and_then(Clone::clone);
                let pre_activation = snapshot.activations.get(&module).and_then(Clone::clone);
                let pre_receipt = snapshot.receipts.get(&module).and_then(Clone::clone);

                match &pre_applied {
                    Some(value) => db.set_json(&format!("applied:{module}"), value)?,
                    None => db.remove_json(&format!("applied:{module}"))?,
                }
                match pre_desired {
                    Some(value) => db.set_json(&format!("desired:{module}"), &value)?,
                    None => db.remove_json(&format!("desired:{module}"))?,
                }
                match pre_health {
                    Some(value) => db.set_json(&format!("health:{module}"), &value)?,
                    None => db.remove_json(&format!("health:{module}"))?,
                }
                match pre_activation {
                    Some(value) => db.set_json(&format!("runtime_activation:{module}"), &value)?,
                    None => db.remove_json(&format!("runtime_activation:{module}"))?,
                }
                match pre_receipt {
                    Some(value) => db.set_json(&format!("apply_receipt:{module}"), &value)?,
                    None => db.remove_json(&format!("apply_receipt:{module}"))?,
                }
                db.remove_json(&format!("artifact_error:{module}"))?;
                db.remove_json(&format!("runtime_recovery_error:{module}"))?;

                let has_preexisting_intent = pending_intents.iter().any(|record| {
                    snapshot
                        .preexisting_intents
                        .contains(&record.op_id.to_string())
                        && record.module == module
                });
                if !has_preexisting_intent
                    && (post_applied != pre_applied || trial_targets.contains_key(&module))
                    && let Some(target) = post_applied
                        .or_else(|| trial_targets.remove(&module))
                        .or_else(|| pre_applied.clone())
                {
                    db.begin_intent(&crate::state::IntentRecord {
                        op_id: uuid::Uuid::new_v4(),
                        module: module.clone(),
                        payload: serde_json::to_value(crate::reconcile::ApplyIntent {
                            previous: pre_applied,
                            target,
                            plan: apsp_adapter_sdk::Plan::Restart,
                        })?,
                    })?;
                }
            }
        }
    } else if !state.previous_installs.is_empty() {
        restore_previous_installs(&config.install_root, &state.previous_installs, ops).await?;
    }
    state.previous_installs.clear();
    Ok(())
}

async fn ready(process: &mut dyn ManagedProcess, config: &Config, version: &str) -> Result<()> {
    let started = Instant::now();
    let startup_secs = if cfg!(windows) { 120 } else { 60 };
    let mut consecutive = 0;
    while started.elapsed() < Duration::from_secs(startup_secs) {
        ensure!(!has_exited(process)?, "new Agent exited during startup");
        if let Ok(status) = crate::transport::status(&config.status_socket).await {
            if let Some(error) = status
                .get("bundle_error")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
            {
                anyhow::bail!("new Agent bundle failed during startup trial: {error}");
            }
            if status["agent_version"] == version
                && status["pid"].as_u64() == Some(u64::from(process.id()))
                && status.get("bundle_ready").and_then(|value| value.as_bool()) == Some(true)
            {
                consecutive += 1;
                if consecutive >= 3 {
                    return Ok(());
                }
            } else {
                consecutive = 0;
            }
        } else {
            consecutive = 0;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    anyhow::bail!("new Agent did not become healthy within {startup_secs} seconds")
}

pub async fn supervise(
    config: Config,
    path: std::path::PathBuf,
    monitor_only: bool,
    ops: Arc<dyn Privileged>,
) -> Result<()> {
    if monitor_only {
        crate::retirement::ensure_monitor_only_allowed(&config)?;
    }
    let root = core_root(&config)?;
    ops.create_dir(
        config.state_db.parent().context("state has no parent")?,
        0o700,
        None,
    )
    .await?;
    let lock = rusqlite::Connection::open(
        config
            .state_db
            .parent()
            .context("state has no parent")?
            .join("supervisor.lock"),
    )?;
    lock.busy_timeout(Duration::ZERO)?;
    lock.execute_batch("BEGIN EXCLUSIVE")?;
    let current = reference(&root.join("current"))?;
    ensure!(
        current.parent() == Some(root.as_path()),
        "Agent reference must point to an installed release"
    );
    let mut state: UpgradeState = if root.join("update-state.json").try_exists()? {
        read_json(&root.join("update-state.json"))?
    } else {
        UpgradeState::default()
    };
    let current = current
        .file_name()
        .and_then(|v| v.to_str())
        .context("Agent release path is invalid")?;
    if let Some(trial) = state.trial.take() {
        tracing::warn!(version=%trial.version,"recovering an interrupted Agent update");
        state.failed(trial.version);
        state.current = state
            .previous
            .clone()
            .context("interrupted upgrade has no previous release")?;
        state.last_error = Some(
            "supervisor restarted during an unconfirmed update; restored previous release".into(),
        );
        restore_trial_state(&config, &mut state, ops.as_ref()).await?;
        ops.atomic_symlink(&root.join("current"), &root.join(&state.current))
            .await?;
    } else {
        state.current = current.into();
    }
    save(&root, &state, ops.as_ref()).await?;
    let mut child = spawn(&root, &state.current, &path, monitor_only, ops.as_ref()).await?;
    let shutdown = async {
        #[cfg(unix)]
        {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            tokio::select! { _=term.recv()=>{}, result=tokio::signal::ctrl_c()=>{result?;} }
        }
        #[cfg(windows)]
        tokio::signal::ctrl_c().await?;
        Ok::<_, anyhow::Error>(())
    };
    tokio::pin!(shutdown);
    let mut crashes = 0u32;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {},
            result = &mut shutdown => { result?; child.terminate().await?; return Ok(()); }
        }
        if has_exited(child.as_mut())? {
            crashes = crashes.saturating_add(1);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs((1u64 << crashes.min(5)).min(30))) => {},
                result = &mut shutdown => { result?; return Ok(()); }
            }
            child = spawn(&root, &state.current, &path, monitor_only, ops.as_ref()).await?;
        }
        let pending: Option<PendingUpgrade> = if root.join("pending-update.json").try_exists()? {
            read_json(&root.join("pending-update.json"))?
        } else {
            None
        };
        let Some(pending) = pending else {
            continue;
        };
        if state.failed_versions.contains(&pending.version)
            || release_version(&pending.version) <= release_version(&state.current)
        {
            ops.write_file(&root.join("pending-update.json"), b"null", 0o600, None)
                .await?;
            continue;
        }
        let validation = async {
            let binary = root.join(&pending.version).join(executable_name());
            verify(
                &binary,
                &pending.version,
                &pending.sha256,
                pending
                    .proof
                    .as_ref()
                    .context("pending Agent update has no signed proof")?,
                &apsp_protocol::release::TrustedKeys::compiled()?,
                ops.as_ref(),
            )
            .await?;
            verify_cache_before_upgrade(&binary, &path, ops.as_ref()).await
        }
        .await;
        // Validation runs asynchronously while the current Agent keeps working.
        // Its terminal retirement must win over any pending update or rejection.
        has_exited(child.as_mut())?;
        if let Err(error) = validation {
            tracing::warn!(version=%pending.version,%error,"Agent update validation failed");
            state.failed(pending.version);
            state.last_error = Some(error.to_string());
            save(&root, &state, ops.as_ref()).await?;
            ops.write_file(&root.join("pending-update.json"), b"null", 0o600, None)
                .await?;
            continue;
        }
        tracing::info!(version=%pending.version,"starting Agent update trial");
        child.terminate().await?;
        // The current Agent may have retired after the preflight observation,
        // including while its final exit was being collected during shutdown.
        has_exited(child.as_mut())?;
        state.previous = Some(state.current.clone());
        state.previous_installs.clear();
        state.trial_snapshot = Some(capture_trial_snapshot(&config)?);
        state.trial = Some(pending.clone());
        save(&root, &state, ops.as_ref()).await?;
        let attempt = match spawn(&root, &pending.version, &path, monitor_only, ops.as_ref()).await
        {
            Ok(mut candidate) => {
                let result = tokio::select! {
                    result = ready(candidate.as_mut(), &config, &pending.version) => result,
                    result = &mut shutdown => { result?; candidate.terminate().await?; return Ok(()); }
                };
                match result {
                    Ok(()) => Ok(candidate),
                    Err(error) => {
                        let _ = candidate.terminate().await;
                        Err(error)
                    }
                }
            }
            Err(error) => Err(error),
        };
        match attempt {
            Ok(candidate) => {
                child = candidate;
                ops.atomic_symlink(&root.join("current"), &root.join(&pending.version))
                    .await?;
                state.current = pending.version;
                state.previous_installs.clear();
                state.trial_snapshot = None;
                tracing::info!(version=%state.current,"Agent update activated");
                state.last_error = None;
                crashes = 0;
            }
            Err(error) if error.downcast_ref::<crate::retirement::Retired>().is_some() => {
                return Err(error);
            }
            Err(error) => {
                tracing::warn!(version=%pending.version,%error,"Agent update failed; restoring previous release");
                state.failed(pending.version);
                state.last_error = Some(error.to_string());
                restore_trial_state(&config, &mut state, ops.as_ref()).await?;
                child = spawn(&root, &state.current, &path, monitor_only, ops.as_ref()).await?;
            }
        }
        state.trial = None;
        save(&root, &state, ops.as_ref()).await?;
        ops.write_file(&root.join("pending-update.json"), b"null", 0o600, None)
            .await?;
    }
}

#[cfg(all(test, unix))]
#[path = "supervisor_tests.rs"]
mod tests;

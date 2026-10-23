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

async fn ready(process: &mut dyn ManagedProcess, config: &Config, version: &str) -> Result<()> {
    let started = Instant::now();
    let startup_secs = if cfg!(windows) { 120 } else { 60 };
    let mut consecutive = 0;
    while started.elapsed() < Duration::from_secs(startup_secs) {
        ensure!(!has_exited(process)?, "new Agent exited during startup");
        if let Ok(status) = crate::transport::status(&config.status_socket).await
            && status["agent_version"] == version
            && status["pid"].as_u64() == Some(u64::from(process.id()))
        {
            consecutive += 1;
            if consecutive >= 3 {
                return Ok(());
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
        state.previous = Some(state.current.clone());
        state.trial = Some(pending.clone());
        save(&root, &state, ops.as_ref()).await?;
        tracing::info!(version=%pending.version,"starting Agent update trial");
        child.terminate().await?;
        // The current Agent may have retired after the preflight observation,
        // including while its final exit was being collected during shutdown.
        has_exited(child.as_mut())?;
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

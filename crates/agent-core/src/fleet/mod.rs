mod operations;
mod runtime_permissions;
mod terminals;

use crate::{Config, SharedState, artifacts::PanelClient};
use anyhow::{Result, ensure};
use apsp_adapter_sdk::{Descriptor, Privileged, ServiceManager};
use apsp_protocol::fleet::{AccessPolicy, JobResult, Work};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::sync::{Mutex, watch};

struct ExecutionControl {
    retirement: Arc<crate::retirement::Retirement>,
    lock: Arc<Mutex<()>>,
    descriptors: Arc<Vec<Descriptor>>,
}

pub(crate) fn local_policy(config: &Config) -> Result<AccessPolicy> {
    let path = config
        .identity_dir
        .parent()
        .unwrap_or(Path::new("/etc/apsp"))
        .join("fleet-policy.json");
    // Checks run on the opened handle, so the file cannot be swapped between the
    // check and the read.
    let file = match open_policy(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AccessPolicy::default());
        }
        result => result?,
    };
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= 32 * 1024,
        "fleet policy must be a bounded regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "fleet policy must be private to its owner"
        );
        // The policy can grant root terminals, so only root may own it.
        ensure!(metadata.uid() == 0, "fleet policy must be owned by root");
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(file, 32 * 1024 + 1), &mut bytes)?;
    ensure!(
        bytes.len() <= 32 * 1024,
        "fleet policy must be a bounded regular file"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(unix)]
fn open_policy(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    // A symbolic link fails to open and a FIFO cannot block the read.
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_policy(path: &Path) -> std::io::Result<std::fs::File> {
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(std::io::Error::other("fleet policy must not be a link"));
    }
    std::fs::File::open(path)
}

pub(crate) fn capabilities(config: &Config) -> Vec<String> {
    let mut result = Vec::new();
    if cfg!(target_os = "linux") {
        result.push(apsp_protocol::fleet::OPERATIONS_CAPABILITY.into());
        result.push("system:network:v1".into());
        if let Ok(policy) = local_policy(config) {
            if policy.runtime_inspection && Path::new("/usr/bin/python3").is_file() {
                result.push(apsp_protocol::fleet::RUNTIME_PREFLIGHT_CAPABILITY.into());
            }
            if !policy.terminal_accounts.is_empty()
                && Path::new("/usr/bin/python3").is_file()
                && Path::new("/run/systemd/system").is_dir()
            {
                result.push(apsp_protocol::fleet::TERMINAL_CAPABILITY.into());
            }
            if (!policy.read_directories.is_empty() || !policy.write_directories.is_empty())
                && Path::new("/usr/bin/python3").is_file()
            {
                result.push("fleet:files:v1".into());
                result.push("fleet:files:transfer:v1".into());
            }
            if !policy.services.is_empty() {
                result.push("fleet:services:v1".into());
            }
            if policy.system_network && Path::new("/run/systemd/system").is_dir() {
                result.push("system:network:apply:v1".into());
            }
            if policy.private_mesh
                && Path::new("/usr/bin/wg").is_file()
                && Path::new("/usr/bin/wg-quick").is_file()
            {
                result.push("system:private-mesh:v1".into());
            }
            if policy.reverse_tunnel
                && Path::new("/usr/bin/ssh").is_file()
                && Path::new("/usr/bin/ssh-keygen").is_file()
                && Path::new("/run/systemd/system").is_dir()
            {
                result.push("system:reverse-tunnel:v1".into());
            }
            if policy.firewall
                && Path::new("/usr/sbin/nft").is_file()
                && Path::new("/run/systemd/system").is_dir()
            {
                result.push("system:firewall:v1".into());
            }
            if policy.port_forward
                && Path::new("/run/systemd/system").is_dir()
                && Path::new("/usr/bin/socat").is_file()
                && Path::new("/usr/bin/ss").is_file()
            {
                result.push("system:forwarding:v1".into());
            }
            if policy.certificate_deploy
                && !policy.write_directories.is_empty()
                && !policy.services.is_empty()
            {
                result.push("system:certificate-deploy:v1".into());
            }
        }
    }
    result
}

pub(crate) async fn run(
    config: Config,
    state: SharedState,
    ops: Arc<dyn Privileged>,
    services: Arc<dyn ServiceManager>,
    mut client: watch::Receiver<Option<Arc<PanelClient>>>,
    retirement: Arc<crate::retirement::Retirement>,
    descriptors: Vec<Descriptor>,
) -> Result<()> {
    let mut terminals = terminals::Sessions::default();
    let execution = ExecutionControl {
        retirement: retirement.clone(),
        lock: Arc::new(Mutex::new(())),
        descriptors: Arc::new(descriptors),
    };
    loop {
        // Local expiry and policy revocation do not depend on successful panel delivery.
        match local_policy(&config) {
            Ok(policy) => terminals.enforce_local(&policy).await,
            Err(_) => terminals.close_all().await,
        }
        terminals.disconnect().await;
        let active = client.borrow_and_update().clone();
        let retired = retirement.requested();
        if retired {
            terminals.close_all().await;
        }
        if !retired && let Some(active) = active {
            if let Err(error) = tick(
                &config,
                &state,
                ops.clone(),
                services.clone(),
                &active,
                &mut terminals,
                &execution,
            )
            .await
            {
                tracing::warn!(%error,"fleet management polling failed");
                terminals.disconnect().await;
            } else {
                terminals.connected();
            }
        } else {
            terminals.disconnect().await;
        }
        let delay = if terminals.has_live() {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(2)
        };
        tokio::select! {_=tokio::time::sleep(delay)=>{},changed=client.changed()=>{changed?;}}
    }
}

async fn tick(
    config: &Config,
    state: &SharedState,
    ops: Arc<dyn Privileged>,
    services: Arc<dyn ServiceManager>,
    client: &PanelClient,
    terminals: &mut terminals::Sessions,
    control: &ExecutionControl,
) -> Result<()> {
    let retirement = &control.retirement;
    let _guard = retirement.gate.read().await;
    if retirement.requested() {
        terminals.close_all().await;
        return Ok(());
    }
    let pending = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
        .get_json::<Vec<JobResult>>("fleet_results")?
        .unwrap_or_default();
    for result in &pending {
        let delivered = tokio::time::timeout(
            Duration::from_secs(2),
            client.post_json_status::<serde_json::Value>("/api/agent/v1/fleet/results", result),
        )
        .await??;
        match delivered {
            Ok(_) => {}
            // A result the panel refuses outright is dropped, so it cannot block every
            // later result and the work fetch below; the panel reconciles the job.
            Err(status) if permanently_rejected(status) => {
                tracing::error!(%status, job = %result.id, "panel rejected a fleet result; dropping it");
            }
            Err(status) => anyhow::bail!("panel did not accept fleet results: HTTP {status}"),
        }
        let mut durable = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        let mut records = durable
            .get_json::<Vec<JobResult>>("fleet_results")?
            .unwrap_or_default();
        records.retain(|record| record.id != result.id);
        durable.set_json("fleet_results", &records)?;
    }
    let work: serde_json::Value = tokio::time::timeout(
        Duration::from_secs(2),
        client.get_json("/api/agent/v1/fleet/work"),
    )
    .await??;
    let work = parse_work(&work, state)?;
    // The panel has already marked fetched jobs dispatched. A broken local policy or
    // terminal update must not drop them: each job reads the policy again and reports
    // its own failure, and terminals fall back to a policy that grants nothing.
    let policy = local_policy(config).unwrap_or_else(|error| {
        tracing::error!(%error, "fleet policy unavailable; terminals are closed");
        AccessPolicy::default()
    });
    if let Err(error) = terminals
        .tick(&work.terminals, &policy, ops.as_ref(), state, client)
        .await
    {
        tracing::error!(%error, "fleet terminal update failed");
    }
    for job in work.jobs {
        let key = format!("fleet_intent:{}", job.id);
        {
            let mut durable = state
                .lock()
                .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
            if durable.get_json::<bool>(&key)?.is_some() {
                tracing::error!(job = %job.id, "operation already attempted; remote state must be reconciled");
                continue;
            }
            durable.set_json(&key, &true)?;
        }
        let config = config.clone();
        let state = state.clone();
        let ops = ops.clone();
        let services = services.clone();
        let retirement = retirement.clone();
        let client = client.clone();
        let execution_lock = control.lock.clone();
        let descriptors = control.descriptors.clone();
        tokio::spawn(async move {
            let result = async {
                let _exclusive = execution_lock.lock().await;
                let _guard = retirement.gate.read().await;
                let execution = if retirement.requested() || job.expires_at <= panel_now(&state) {
                    Err(anyhow::anyhow!("operation expired before execution"))
                } else {
                    match local_policy(&config) {
                        Ok(policy) => {
                            operations::execute(
                                &config,
                                &policy,
                                &job,
                                ops.as_ref(),
                                services.as_ref(),
                                &client,
                                &descriptors,
                            )
                            .await
                        }
                        Err(error) => Err(error),
                    }
                };
                let result = match execution {
                    Ok(result) => JobResult {
                        id: job.id,
                        succeeded: true,
                        result,
                        error: None,
                        completed_at: panel_now(&state),
                    },
                    Err(error) => JobResult {
                        id: job.id,
                        succeeded: false,
                        result: serde_json::Value::Null,
                        error: Some(error.to_string().chars().take(2048).collect()),
                        completed_at: panel_now(&state),
                    },
                };
                {
                    let mut durable = state
                        .lock()
                        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
                    let mut records = durable
                        .get_json::<Vec<JobResult>>("fleet_results")?
                        .unwrap_or_default();
                    ensure!(records.len() < 32, "fleet result outbox full");
                    records.push(result);
                    durable.set_json("fleet_results", &records)?;
                    durable.remove_json(&key)?;
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                tracing::error!(%error,"fleet outcome could not be persisted; operation will not be replayed");
            }
        });
    }
    Ok(())
}

/// Seconds on the panel clock; device clocks may drift and the panel rejects
/// results that claim to finish in its future.
fn panel_now(state: &SharedState) -> i64 {
    let offset = state
        .lock()
        .ok()
        .and_then(|state| state.get_json::<i64>("clock_offset_ms").ok().flatten())
        .unwrap_or(0);
    apsp_protocol::now_timestamp().saturating_add(offset / 1000)
}

fn permanently_rejected(status: reqwest::StatusCode) -> bool {
    status.is_client_error()
        && !matches!(
            status,
            reqwest::StatusCode::UNAUTHORIZED
                | reqwest::StatusCode::REQUEST_TIMEOUT
                | reqwest::StatusCode::TOO_MANY_REQUESTS
        )
}

/// Parses each job and terminal separately, so a job an older Agent cannot read
/// fails on its own instead of losing the whole batch.
fn parse_work(value: &serde_json::Value, state: &SharedState) -> Result<Work> {
    let mut work = Work::default();
    for control in value["terminals"].as_array().into_iter().flatten() {
        match serde_json::from_value(control.clone()) {
            Ok(control) => work.terminals.push(control),
            Err(error) => tracing::error!(%error, "unsupported terminal control ignored"),
        }
    }
    for job in value["jobs"].as_array().into_iter().flatten() {
        match serde_json::from_value(job.clone()) {
            Ok(job) => work.jobs.push(job),
            Err(error) => {
                let Some(id) = job["id"].as_str().and_then(|id| id.parse().ok()) else {
                    tracing::error!(%error, "unreadable fleet job ignored");
                    continue;
                };
                // Read the panel clock before taking the state lock; it is not reentrant.
                let completed_at = panel_now(state);
                let mut durable = state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
                let mut records = durable
                    .get_json::<Vec<JobResult>>("fleet_results")?
                    .unwrap_or_default();
                if records.len() < 32 && !records.iter().any(|record| record.id == id) {
                    records.push(JobResult {
                        id,
                        succeeded: false,
                        result: serde_json::Value::Null,
                        error: Some(format!(
                            "this Agent version cannot read the operation and did not run it: {error}"
                        )
                        .chars()
                        .take(2048)
                        .collect()),
                        completed_at,
                    });
                    durable.set_json("fleet_results", &records)?;
                }
            }
        }
    }
    Ok(work)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::State;
    use std::sync::Mutex;

    #[test]
    fn unreadable_jobs_fail_alone_and_readable_ones_still_run() -> Result<()> {
        let state = Arc::new(Mutex::new(State::open(Path::new(":memory:"))?));
        let readable = uuid::Uuid::new_v4();
        let unreadable = uuid::Uuid::new_v4();
        let work = serde_json::json!({
            "jobs": [
                {"id": readable, "operation": {"kind": "snapshot"},
                 "policy": AccessPolicy::default(), "expires_at": 1},
                {"id": unreadable, "operation": {"kind": "operation_from_a_newer_panel"},
                 "policy": AccessPolicy::default(), "expires_at": 1},
                {"operation": {"kind": "snapshot"}}
            ],
            "terminals": [{"unexpected": true}]
        });
        let parsed = parse_work(&work, &state)?;
        assert_eq!(parsed.jobs.len(), 1);
        assert_eq!(parsed.jobs[0].id, readable);
        assert!(parsed.terminals.is_empty());
        let outbox = state
            .lock()
            .unwrap()
            .get_json::<Vec<JobResult>>("fleet_results")?
            .unwrap_or_default();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].id, unreadable);
        assert!(!outbox[0].succeeded);
        // Reading the same batch again does not queue a second result.
        parse_work(&work, &state)?;
        let outbox = state
            .lock()
            .unwrap()
            .get_json::<Vec<JobResult>>("fleet_results")?
            .unwrap_or_default();
        assert_eq!(outbox.len(), 1);
        Ok(())
    }

    #[test]
    fn only_definite_refusals_drop_a_queued_result() {
        assert!(permanently_rejected(reqwest::StatusCode::BAD_REQUEST));
        assert!(permanently_rejected(reqwest::StatusCode::CONFLICT));
        assert!(!permanently_rejected(reqwest::StatusCode::UNAUTHORIZED));
        assert!(!permanently_rejected(
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(!permanently_rejected(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
    }
}

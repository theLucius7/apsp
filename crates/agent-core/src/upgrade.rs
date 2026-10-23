mod supervisor;
#[cfg(all(test, unix))]
mod tests;
use crate::{
    Config, SharedState,
    artifacts::PanelClient,
    system::deploy::{core_root, executable_name},
};
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::Privileged;
use apsp_protocol::{AgentRelease, AgentSettings, release_version};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path, sync::Arc, time::Duration};
pub use supervisor::supervise;
use tokio::{sync::watch, time::Instant};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingUpgrade {
    pub version: String,
    pub sha256: String,
    #[serde(default)]
    pub proof: Option<apsp_protocol::release::ReleaseProof>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UpgradeState {
    pub current: String,
    pub previous: Option<String>,
    pub trial: Option<PendingUpgrade>,
    pub failed_versions: BTreeSet<String>,
    pub last_error: Option<String>,
}

impl UpgradeState {
    fn failed(&mut self, version: String) {
        self.failed_versions.insert(version);
        if self.failed_versions.len() > 32 {
            let oldest = self
                .failed_versions
                .iter()
                .min_by_key(|v| release_version(v))
                .cloned();
            if let Some(oldest) = oldest {
                self.failed_versions.remove(&oldest);
            }
        }
    }
}

pub fn state(config: &Config) -> Result<Option<UpgradeState>> {
    let path = core_root(config)?.join("update-state.json");
    if path.try_exists()? {
        Ok(Some(read_json(&path)?))
    } else {
        Ok(None)
    }
}

pub async fn run(
    config: Config,
    state: SharedState,
    ops: Arc<dyn Privileged>,
    clients: watch::Receiver<Option<Arc<PanelClient>>>,
    retirement: Arc<crate::retirement::Retirement>,
    agent_version: &'static str,
) -> Result<()> {
    let mut next = Instant::now();
    loop {
        {
            let _guard = retirement.gate.read().await;
            if !retirement.requested() {
                let settings = state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
                    .get_json::<AgentSettings>("agent_settings")?
                    .unwrap_or_else(|| config.settings.clone());
                if !settings.auto_update {
                    next = Instant::now();
                } else if Instant::now() >= next {
                    let active = clients.borrow().clone();
                    if let Some(active) = active {
                        match check_guarded(
                            &config,
                            ops.as_ref(),
                            &active,
                            agent_version,
                            Some(&retirement),
                        )
                        .await
                        {
                            Ok(()) => {
                                next = Instant::now()
                                    + Duration::from_secs(6 * 3600 + rand::random::<u64>() % 300)
                            }
                            Err(error) => {
                                tracing::warn!(%error,"Agent update check failed");
                                next = Instant::now() + Duration::from_secs(300);
                            }
                        }
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

#[cfg(all(test, unix))]
async fn check(config: &Config, ops: &dyn Privileged, client: &PanelClient) -> Result<()> {
    check_guarded(config, ops, client, env!("CARGO_PKG_VERSION"), None).await
}

async fn check_guarded(
    config: &Config,
    ops: &dyn Privileged,
    client: &PanelClient,
    agent_version: &str,
    retirement: Option<&crate::retirement::Retirement>,
) -> Result<()> {
    let active = || -> Result<()> {
        ensure!(
            !retirement.is_some_and(|retirement| retirement.requested()),
            "Agent is retiring; automatic update was cancelled"
        );
        Ok(())
    };
    active()?;
    let root = core_root(config)?;
    ensure!(
        root.join("update-state.json").is_file(),
        "automatic updates require the installed Agent supervisor"
    );
    let release: Option<AgentRelease> = client
        .get_json("/api/agent/v1/update?download_source=github")
        .await?;
    active()?;
    let Some(release) = release else {
        return Ok(());
    };
    ensure!(
        release_version(&release.version) > release_version(agent_version),
        "update must be a newer stable version"
    );
    let previous: UpgradeState = read_json(&root.join("update-state.json"))?;
    if previous.failed_versions.contains(&release.version) || previous.trial.is_some() {
        return Ok(());
    }
    let bytes = client
        .agent_binary(
            &release.artifact,
            &release.version,
            &release.download_mirror,
        )
        .await?;
    active()?;
    ensure!(
        valid_executable(&bytes),
        "Agent update has the wrong executable format or architecture"
    );
    let directory = root.join(&release.version);
    let binary = directory.join(executable_name());
    active()?;
    ops.create_dir(&directory, 0o755, None).await?;
    active()?;
    if binary.try_exists()? {
        ensure!(
            std::fs::read(&binary)? == bytes,
            "Agent version already contains different bytes"
        );
    } else {
        ops.write_file(&binary, &bytes, 0o755, None).await?;
    }
    active()?;
    let proof = release
        .artifact
        .proof
        .as_ref()
        .context("Agent update lacks signed proof")?;
    for (name, bytes) in [
        ("release.json", proof.metadata_json.as_bytes()),
        ("SHA256SUMS", proof.checksums.as_bytes()),
        ("SHA256SUMS.minisig", proof.signature.as_bytes()),
    ] {
        active()?;
        ops.write_file(&directory.join(name), bytes, 0o644, None)
            .await?;
    }
    verify_guarded(
        &binary,
        &release.version,
        &release.artifact.sha256,
        proof,
        client.keys()?,
        ops,
        retirement,
    )
    .await?;
    active()?;
    let pending = PendingUpgrade {
        version: release.version,
        sha256: release.artifact.sha256,
        proof: release.artifact.proof,
    };
    ops.write_file(
        &root.join("pending-update.json"),
        &serde_json::to_vec(&Some(pending))?,
        0o600,
        None,
    )
    .await?;
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 128 * 1024,
        "invalid update state file"
    );
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

async fn verify(
    binary: &Path,
    version: &str,
    digest: &str,
    proof: &apsp_protocol::release::ReleaseProof,
    keys: &apsp_protocol::release::TrustedKeys,
    ops: &dyn Privileged,
) -> Result<()> {
    verify_guarded(binary, version, digest, proof, keys, ops, None).await
}

async fn verify_cache_before_upgrade(
    binary: &Path,
    config_path: &Path,
    ops: &dyn Privileged,
) -> Result<()> {
    // The candidate has already been authenticated with the running Agent's
    // roots. Its own roots must also accept every existing cache reference
    // before the supervisor stops the current process or records a trial.
    let output = ops
        .execute_bounded(
            binary,
            &[
                "--config".into(),
                config_path.to_string_lossy().into_owned(),
                "verify-cache".into(),
            ],
            120,
            16 * 1024,
        )
        .await?;
    ensure!(
        output.output.success && !output.timed_out,
        "Agent update cache signature preflight failed; current Agent was not stopped"
    );
    Ok(())
}

async fn verify_guarded(
    binary: &Path,
    version: &str,
    digest: &str,
    proof: &apsp_protocol::release::ReleaseProof,
    keys: &apsp_protocol::release::TrustedKeys,
    ops: &dyn Privileged,
    retirement: Option<&crate::retirement::Retirement>,
) -> Result<()> {
    ensure!(
        release_version(version).is_some(),
        "invalid Agent release version"
    );
    let metadata = std::fs::symlink_metadata(binary)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 128 * 1024 * 1024,
        "invalid Agent binary"
    );
    ensure!(
        format!("{:x}", Sha256::digest(std::fs::read(binary)?)) == digest.to_ascii_lowercase(),
        "Agent update SHA256 mismatch"
    );
    let release = apsp_protocol::release::verify_release(proof, keys)?;
    let verified = release.native_artifact("agent", version)?;
    ensure!(
        verified.metadata().format == "raw"
            && verified.metadata().binary_name == executable_name()
            && verified.sha256() == digest.to_ascii_lowercase(),
        "Agent update signed identity mismatch"
    );
    verified.verify_binary(&std::fs::read(binary)?)?;
    crate::artifacts::verify_expected_agent(binary, version, keys).await?;
    ensure!(
        !retirement.is_some_and(|retirement| retirement.requested()),
        "Agent is retiring; candidate execution was cancelled"
    );
    let output = ops
        .execute_bounded(binary, &["--version".into()], 10, 1024)
        .await?;
    ensure!(
        output.output.success
            && !output.timed_out
            && output.output.stdout.trim() == format!("apsp-agent {version}"),
        "Agent update version check failed"
    );
    Ok(())
}

fn valid_executable(bytes: &[u8]) -> bool {
    let arm = std::env::consts::ARCH == "aarch64";
    if cfg!(windows) {
        if bytes.len() < 64 || &bytes[..2] != b"MZ" {
            return false;
        }
        let offset = u32::from_le_bytes(bytes[60..64].try_into().expect("header width")) as usize;
        let Some(header) = bytes.get(offset..offset.saturating_add(6)) else {
            return false;
        };
        &header[..4] == b"PE\0\0"
            && u16::from_le_bytes([header[4], header[5]]) == if arm { 0xaa64 } else { 0x8664 }
    } else if cfg!(target_os = "macos") {
        bytes.len() >= 8
            && &bytes[..4] == b"\xcf\xfa\xed\xfe"
            && u32::from_le_bytes(bytes[4..8].try_into().expect("header width")) == 0x100000c
    } else {
        bytes.len() >= 20
            && &bytes[..4] == b"\x7fELF"
            && bytes[4] == 2
            && bytes[5] == 1
            && u16::from_le_bytes([bytes[18], bytes[19]]) == if arm { 183 } else { 62 }
    }
}

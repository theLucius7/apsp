use super::{ensure_ordinary_directory_if_present, verification};
use crate::{Config, reconcile::ApplyIntent};
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::{DiagnosticSpec, Prepared, ServiceJob};
use apsp_protocol::{
    DiagnosticJob,
    release::{TrustedKeys, safe_component},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;
use std::{
    path::{Component, PathBuf},
    time::Duration,
};

struct Reference {
    binary: PathBuf,
    version: String,
    artifact_name: Option<String>,
}

impl From<Prepared> for Reference {
    fn from(prepared: Prepared) -> Self {
        Self {
            binary: prepared.spec.binary_path,
            version: prepared.spec.kernel_version,
            artifact_name: None,
        }
    }
}

#[derive(Deserialize)]
enum DiagnosticCheckpoint {
    Preparing(DiagnosticJob),
    Started {
        spec: DiagnosticSpec,
        service: ServiceJob,
        // The persisted diagnostic checkpoint predates the internal tool terminology.
        #[serde(rename = "plugin")]
        tool: String,
    },
}

// Read only: do not invoke State::open, migrations, chmod, or a writing PRAGMA.
fn saved_references(config: &Config, keys: &TrustedKeys) -> Result<Vec<Reference>> {
    let mut references = Vec::new();
    match std::fs::symlink_metadata(&config.state_db) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(references),
        result => ensure!(result?.is_file(), "state database must be an ordinary file"),
    }
    let wal = PathBuf::from(format!("{}-wal", config.state_db.display()));
    let shm = PathBuf::from(format!("{}-shm", config.state_db.display()));
    for sidecar in [&wal, &shm] {
        match std::fs::symlink_metadata(sidecar) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            result => ensure!(
                result?.is_file(),
                "database sidecar must be an ordinary file"
            ),
        }
    }
    if std::fs::symlink_metadata(&wal).is_ok_and(|metadata| metadata.len() > 0) {
        ensure!(
            std::fs::symlink_metadata(&shm).is_ok_and(|metadata| metadata.is_file()),
            "read-only preflight requires the existing WAL shared-memory file"
        );
    }
    let mut connection = Connection::open_with_flags(
        &config.state_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(10))?;
    let transaction = connection.transaction()?;
    {
        let mut statement =
            transaction.prepare("SELECT value FROM kv WHERE key LIKE 'applied:%' ORDER BY key")?;
        for row in statement.query_map([], |row| row.get::<_, String>(0))? {
            let prepared: Prepared = serde_json::from_str(&row?)
                .context("decode applied runtime for artifact preflight")?;
            references.push(prepared.into());
        }
    }
    {
        let mut statement = transaction
            .prepare("SELECT payload FROM intents WHERE completed = 0 ORDER BY rowid")?;
        for row in statement.query_map([], |row| row.get::<_, String>(0))? {
            let intent: ApplyIntent = serde_json::from_str(&row?)
                .context("decode pending runtime for artifact preflight")?;
            references.push(intent.target.into());
            if let Some(previous) = intent.previous {
                references.push(previous.into());
            }
        }
    }
    let active: Option<String> = transaction
        .query_row(
            "SELECT value FROM kv WHERE key = 'diagnostics:active'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(active) = active {
        match serde_json::from_str::<Option<DiagnosticCheckpoint>>(&active)? {
            Some(DiagnosticCheckpoint::Started {
                spec,
                service,
                tool,
            }) => {
                ensure!(
                    service.program == spec.binary_path,
                    "diagnostic checkpoint executes an unexpected binary"
                );
                let self_exe = crate::bundled::current_agent_binary().ok();
                if spec.binary_path.starts_with(&config.agent_root)
                    || self_exe.as_ref() == Some(&spec.binary_path)
                {
                    if spec.binary_path.starts_with(&config.agent_root) {
                        references.push(Reference {
                            binary: spec.binary_path,
                            version: spec.version,
                            artifact_name: Some("agent".into()),
                        });
                    }
                } else {
                    references.push(Reference {
                        binary: spec.binary_path,
                        version: spec.version,
                        artifact_name: Some(tool),
                    });
                }
            }
            Some(DiagnosticCheckpoint::Preparing(job)) => {
                if let Some(job_artifact) = &job.artifact {
                    let release = verification::signed_release(
                        job_artifact
                            .proof
                            .as_ref()
                            .context("pending diagnostic artifact has no signed proof")?,
                        keys,
                    )?;
                    let artifact =
                        crate::runtime_platform::artifact(&release, &job.tool, &job.version)?;
                    ensure!(
                        artifact.metadata().format == "tar.gz",
                        "pending diagnostic artifact has an unsupported format"
                    );
                    ensure!(
                        artifact.sha256() == job_artifact.sha256,
                        "pending diagnostic checksum differs from its signed release"
                    );
                    let binary = config
                        .install_root
                        .join(&job.tool)
                        .join(&job.version)
                        .join(&artifact.metadata().binary_name);
                    if std::fs::symlink_metadata(
                        binary.parent().context("diagnostic binary has no parent")?,
                    )
                    .is_ok()
                    {
                        references.push(Reference {
                            binary,
                            version: job.version,
                            artifact_name: Some(job.tool),
                        });
                    }
                }
            }
            None => {}
        }
    }
    // Dropping the read transaction releases the snapshot without modifying persistent state.
    Ok(references)
}

async fn verify_reference(
    config: &Config,
    reference: &Reference,
    keys: &TrustedKeys,
) -> Result<()> {
    if reference.artifact_name.as_deref() == Some("agent")
        && reference.binary.starts_with(&config.agent_root)
    {
        return verification::verify_binary_with_keys(
            &reference.binary,
            "agent",
            "raw",
            keys,
            None,
        )
        .await;
    }
    let version_dir = reference
        .binary
        .parent()
        .context("cached binary has no version directory")?;
    let artifact_dir = version_dir
        .parent()
        .context("cached binary has no artifact directory")?;
    let artifact_name = artifact_dir
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid cached artifact path")?;
    let dir_name = version_dir
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid cached version directory")?;
    if let Some(manifest) = crate::bundled::read_bundled_manifest(version_dir).await? {
        ensure!(
            reference.binary.is_absolute()
                && artifact_dir.parent() == Some(config.install_root.as_path())
                && safe_component(artifact_name)
                && apsp_protocol::bundled::valid_version_segment(&reference.version)
                && manifest.component_name == artifact_name
                && manifest.version == reference.version
                && crate::bundled::matches_version_directory_name(dir_name, &reference.version)
                && reference.binary.file_name().and_then(|name| name.to_str())
                    == Some(manifest.binary_name.as_str())
                && reference
                    .artifact_name
                    .as_ref()
                    .is_none_or(|expected| expected == artifact_name),
            "cached bundled component path differs from its saved identity"
        );
        return verification::verify_installed_bundle(
            &config.agent_root,
            version_dir,
            &manifest,
            keys,
        )
        .await;
    }
    ensure!(
        reference.binary.is_absolute()
            && artifact_dir.parent() == Some(config.install_root.as_path())
            && safe_component(artifact_name)
            && safe_component(&reference.version)
            && dir_name == reference.version
            && reference
                .artifact_name
                .as_ref()
                .is_none_or(|expected| expected == artifact_name),
        "cached artifact path differs from its saved identity"
    );
    let proof = verification::read_proof(version_dir).await?;
    let release = verification::signed_release(&proof, keys)?;
    let artifact = crate::runtime_platform::artifact(&release, artifact_name, &reference.version)?;
    ensure!(
        artifact.metadata().format == "tar.gz",
        "cached artifact has an unsupported format"
    );
    ensure!(
        reference.binary.file_name().and_then(|name| name.to_str())
            == Some(artifact.metadata().binary_name.as_str()),
        "cached executable name differs from signed release"
    );
    verification::verify_file(&reference.binary, &artifact).await
}

async fn current_references(config: &Config, keys: &TrustedKeys) -> Result<Vec<Reference>> {
    let mut references = Vec::new();
    let mut artifacts = match tokio::fs::read_dir(&config.install_root).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(references),
        result => result?,
    };
    while let Some(artifact_dir) = artifacts.next_entry().await? {
        let kind = artifact_dir.file_type().await?;
        ensure!(
            !kind.is_symlink(),
            "cached artifact directory cannot be a symlink"
        );
        if !kind.is_dir() {
            continue;
        }
        let current = artifact_dir.path().join("current");
        match tokio::fs::symlink_metadata(&current).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            result => {
                let metadata = result?;
                #[cfg(unix)]
                ensure!(
                    metadata.file_type().is_symlink(),
                    "current artifact path must be a controlled symlink"
                );
                #[cfg(windows)]
                ensure!(
                    metadata.is_file() && !metadata.file_type().is_symlink(),
                    "current artifact path must be a controlled reference"
                );
            }
        }
        let resolved = tokio::fs::canonicalize(crate::system::deploy::reference(&current)?).await?;
        ensure!(
            resolved.parent()
                == Some(
                    tokio::fs::canonicalize(artifact_dir.path())
                        .await?
                        .as_path()
                ),
            "current artifact link escapes its installation directory"
        );
        let version = resolved
            .file_name()
            .and_then(|name| name.to_str())
            .context("invalid current artifact version")?;
        let artifact_name = artifact_dir
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("invalid current artifact name"))?;
        if let Some(manifest) = crate::bundled::read_bundled_manifest(&resolved).await? {
            references.push(Reference {
                binary: artifact_dir
                    .path()
                    .join(version)
                    .join(&manifest.binary_name),
                version: manifest.version,
                artifact_name: Some(artifact_name),
            });
            continue;
        }
        let proof = verification::read_proof(&resolved).await?;
        let release = verification::signed_release(&proof, keys)?;
        let artifact = crate::runtime_platform::artifact(&release, &artifact_name, version)?;
        references.push(Reference {
            binary: artifact_dir
                .path()
                .join(version)
                .join(&artifact.metadata().binary_name),
            version: version.to_owned(),
            artifact_name: Some(artifact_name),
        });
    }
    Ok(references)
}

pub(super) async fn verify_cache_with_keys(config: &Config, keys: &TrustedKeys) -> Result<()> {
    ensure!(
        config.install_root.is_absolute()
            && config.install_root.components().all(|component| matches!(
                component,
                Component::RootDir | Component::Normal(_) | Component::Prefix(_)
            )),
        "invalid artifact installation root"
    );
    ensure_ordinary_directory_if_present(&config.install_root).await?;
    ensure_ordinary_directory_if_present(
        config
            .state_db
            .parent()
            .context("state database has no parent")?,
    )
    .await?;
    let mut references = saved_references(config, keys)?;
    references.extend(current_references(config, keys).await?);
    for reference in references {
        verify_reference(config, &reference, keys)
            .await
            .with_context(|| format!("verify cached artifact {}", reference.binary.display()))?;
    }
    Ok(())
}

/// Checks all applied, pending, and current cached executables before switching Agent versions.
/// This reads the existing database without migrations or persistent state changes.
pub async fn verify_cache(config: &Config) -> Result<()> {
    verify_cache_with_keys(config, &TrustedKeys::compiled()?).await
}

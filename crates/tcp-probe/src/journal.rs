use crate::{INPUT_LIMIT, OUTPUT_LIMIT, Options, Report, Snapshot, model::now_millis};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    time::{Instant, timeout, timeout_at},
};

const IO_LIMIT: Duration = Duration::from_secs(2);

mod pending;
use pending::PendingFile;

pub struct Journal {
    workspace: PathBuf,
    targets_file: String,
    digest: String,
    snapshot: Snapshot,
    revisions: std::collections::BTreeMap<String, u64>,
    #[cfg(unix)]
    owner_uid: u32,
    #[cfg(test)]
    publication_gate: std::sync::Mutex<Option<PublicationGate>>,
    #[cfg(test)]
    publication_fault: std::sync::Mutex<Option<PublicationFault>>,
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum PublicationFault {
    Write,
    Sync,
    Rename,
}

#[cfg(test)]
struct PublicationGate {
    writes_before: usize,
    entered: std::sync::Arc<tokio::sync::Notify>,
    release: std::sync::Arc<tokio::sync::Notify>,
    probe_deadline: std::sync::Arc<std::sync::Mutex<Option<Instant>>>,
}

#[derive(Serialize)]
struct Section<'a> {
    name: &'a str,
    text: String,
    complete: bool,
    revision: u64,
    collected_at: u64,
}

fn private_metadata(metadata: &std::fs::Metadata, directory: bool) -> Result<()> {
    ensure!(
        !metadata.file_type().is_symlink(),
        "symlinks are not permitted"
    );
    ensure!(
        if directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        },
        "workspace/input must be an ordinary directory/file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "workspace/input must be private"
        );
        if !directory {
            ensure!(metadata.nlink() == 1, "input hard links are not permitted");
        }
        Ok(())
    }
    #[cfg(not(unix))]
    anyhow::bail!("private workspace verification requires Unix")
}

async fn regular_private(path: &Path, directory: bool) -> Result<std::fs::Metadata> {
    let metadata = fs::symlink_metadata(path).await?;
    private_metadata(&metadata, directory)?;
    Ok(metadata)
}

impl Journal {
    pub async fn open(options: &Options) -> Result<Self> {
        ensure!(options.valid(), "invalid probe options");
        timeout(IO_LIMIT, async {
            let _workspace_metadata = regular_private(&options.workspace, true).await?;
            #[cfg(unix)]
            let owner_uid = {
                use std::os::unix::fs::MetadataExt;
                _workspace_metadata.uid()
            };
            ensure!(
                fs::canonicalize(&options.workspace).await? == options.workspace,
                "workspace ancestors must not be symlinks"
            );
            for file in ["result.json", "sections"] {
                match fs::symlink_metadata(options.workspace.join(file)).await {
                    Err(error) if error.kind() == ErrorKind::NotFound => (),
                    _ => anyhow::bail!("workspace already contains diagnostic output"),
                }
            }
            let input = options.workspace.join(&options.targets_file);
            regular_private(&input, false).await?;
            let file = fs::File::open(&input).await?;
            let input_metadata = file.metadata().await?;
            private_metadata(&input_metadata, false)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                ensure!(
                    input_metadata.uid() == owner_uid,
                    "input ownership mismatch"
                );
            }
            ensure!(
                input_metadata.len() <= INPUT_LIMIT as u64,
                "target snapshot exceeds 16 KiB"
            );
            let mut bytes = Vec::new();
            file.take(INPUT_LIMIT as u64 + 1)
                .read_to_end(&mut bytes)
                .await?;
            ensure!(bytes.len() <= INPUT_LIMIT, "target snapshot exceeds 16 KiB");
            ensure!(
                format!("{:x}", Sha256::digest(&bytes)) == options.target_digest,
                "target snapshot digest mismatch"
            );
            let snapshot: Snapshot =
                serde_json::from_slice(&bytes).context("invalid target snapshot")?;
            ensure!(snapshot.valid(), "invalid target snapshot");
            let directory = options.workspace.join("sections");
            tokio::task::spawn_blocking(move || -> Result<()> {
                let mut builder = std::fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(directory)?;
                Ok(())
            })
            .await??;
            Ok(Self {
                workspace: options.workspace.clone(),
                targets_file: options.targets_file.clone(),
                digest: options.target_digest.clone(),
                snapshot,
                revisions: Default::default(),
                #[cfg(unix)]
                owner_uid,
                #[cfg(test)]
                publication_gate: Default::default(),
                #[cfg(test)]
                publication_fault: Default::default(),
            })
        })
        .await
        .context("workspace/input inspection timed out")?
    }

    #[cfg(test)]
    pub(crate) fn gate_publication_after(
        &mut self,
        writes_before: usize,
        entered: std::sync::Arc<tokio::sync::Notify>,
        release: std::sync::Arc<tokio::sync::Notify>,
    ) -> std::sync::Arc<std::sync::Mutex<Option<Instant>>> {
        let probe_deadline = std::sync::Arc::new(std::sync::Mutex::new(None));
        self.publication_gate = std::sync::Mutex::new(Some(PublicationGate {
            writes_before,
            entered,
            release,
            probe_deadline: probe_deadline.clone(),
        }));
        probe_deadline
    }

    #[cfg(test)]
    pub(crate) fn record_probe_deadline(&self, deadline: Instant) {
        if let Some(gate) = self
            .publication_gate
            .lock()
            .expect("publication gate")
            .as_ref()
        {
            *gate.probe_deadline.lock().expect("probe deadline") = Some(deadline);
        }
    }

    pub(crate) fn snapshot(&self, options: &Options) -> Result<Snapshot> {
        ensure!(
            options.valid()
                && options.workspace == self.workspace
                && options.targets_file == self.targets_file
                && options.target_digest == self.digest,
            "workspace/input identity changed"
        );
        Ok(self.snapshot.clone())
    }

    pub(crate) async fn update(
        &mut self,
        report: &Report,
        index: Option<usize>,
        deadline: Instant,
    ) -> Result<()> {
        if let Some(index) = index {
            let target = &report.targets[index];
            let name = format!(
                "tcp_target_{}",
                target.target.id.replace('-', "").to_ascii_lowercase()
            );
            self.section(&name, target, target.complete, deadline)
                .await?;
        } else {
            self.section("tcp_scope", report, true, deadline).await?;
        }
        self.section("tcp_summary", report, report.complete, deadline)
            .await?;
        let bytes = serde_json::to_vec_pretty(report)?;
        ensure!(bytes.len() <= OUTPUT_LIMIT, "report exceeds 64 KiB");
        self.atomic(&self.workspace.join("result.json"), &bytes, deadline)
            .await
    }

    async fn section(
        &mut self,
        name: &str,
        value: &impl Serialize,
        complete: bool,
        deadline: Instant,
    ) -> Result<()> {
        let revision = self.revisions.entry(name.into()).or_default();
        *revision += 1;
        let section = Section {
            name,
            text: serde_json::to_string_pretty(value)?,
            complete,
            revision: *revision,
            collected_at: now_millis()? / 1000,
        };
        let bytes = serde_json::to_vec(&section)?;
        ensure!(bytes.len() <= OUTPUT_LIMIT, "section exceeds 64 KiB");
        self.atomic(
            &self.workspace.join("sections").join(format!("{name}.json")),
            &bytes,
            deadline,
        )
        .await
    }

    async fn atomic(&self, target: &Path, bytes: &[u8], deadline: Instant) -> Result<()> {
        let filename = target
            .file_name()
            .and_then(|name| name.to_str())
            .context("invalid report file name")?;
        let pending_path = target.with_file_name(format!(".{filename}.pending"));
        let deadline = deadline.min(Instant::now() + IO_LIMIT);
        ensure!(Instant::now() < deadline, "report publication timed out");
        let mut pending = PendingFile::create(&pending_path)?;
        let operation = async {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                // The retained create_new handle belongs to the effective user.
                // Reject foreign-owned workspaces before writing any report data.
                ensure!(
                    pending.metadata()?.uid() == self.owner_uid,
                    "workspace must belong to the effective user"
                );
            }
            #[cfg(test)]
            self.fail_publication(PublicationFault::Write)?;
            pending
                .file
                .as_mut()
                .expect("pending writer")
                .write_all(bytes)
                .await?;
            #[cfg(test)]
            {
                let gate = {
                    let mut slot = self.publication_gate.lock().expect("publication gate");
                    match slot.as_mut() {
                        Some(gate) if gate.writes_before > 0 => {
                            gate.writes_before -= 1;
                            None
                        }
                        Some(_) => slot.take(),
                        None => None,
                    }
                };
                if let Some(gate) = gate {
                    gate.entered.notify_one();
                    gate.release.notified().await;
                }
            }
            #[cfg(test)]
            self.fail_publication(PublicationFault::Sync)?;
            pending
                .file
                .as_mut()
                .expect("pending writer")
                .sync_all()
                .await?;
            Ok::<_, anyhow::Error>(())
        };
        let result = timeout_at(deadline, operation)
            .await
            .context("report publication timed out")
            .and_then(|result| result)
            .and_then(|()| {
                ensure!(Instant::now() < deadline, "report publication timed out");
                #[cfg(test)]
                self.fail_publication(PublicationFault::Rename)?;
                pending.publish(target)
            });
        // Report cleanup errors without losing the publication's original error.
        match (result, pending.cleanup()) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(cleanup)) => {
                Err(error.context(format!("pending cleanup failed: {cleanup:#}")))
            }
        }
    }

    #[cfg(test)]
    fn fail_publication(&self, point: PublicationFault) -> Result<()> {
        let mut fault = self.publication_fault.lock().expect("publication fault");
        if *fault == Some(point) {
            fault.take();
            anyhow::bail!("injected publication I/O failure");
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests;

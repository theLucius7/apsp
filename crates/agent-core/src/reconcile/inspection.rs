use super::Reconciler;
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::{Prepared, RuntimeInstance};
use apsp_protocol::Bundle;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};
use tokio::io::AsyncReadExt;

const MAX_CONFIG_BYTES: usize = 32 * 1024 * 1024;
const MAX_CONFIG_FILES: usize = 512;

impl Reconciler {
    /// Inspect both the controlled instance and the signed/configuration bytes around health.
    pub(super) async fn inspect_runtime(&self, runtime: &Prepared) -> Result<RuntimeInstance> {
        self.bounded(async {
            ensure!(
                self.services.supports_runtime_checkpoint(),
                "exact runtime inspection is unsupported"
            );
            let unit = self.adapter.describe().service_unit;
            let first = self.services.runtime_instance(&unit).await?;
            self.inspect_configuration(runtime, &first).await?;
            self.verify_runtime(runtime).await?;
            ensure!(
                self.runtime_health(runtime).await?,
                "runtime is not healthy"
            );
            let second = self.services.runtime_instance(&unit).await?;
            ensure!(
                first == second,
                "runtime instance changed during inspection"
            );
            self.inspect_configuration(runtime, &second).await?;
            self.verify_runtime(runtime).await?;
            Ok(second)
        })
        .await
    }

    async fn inspect_configuration(
        &self,
        runtime: &Prepared,
        instance: &RuntimeInstance,
    ) -> Result<()> {
        let descriptor = self.adapter.describe();
        ensure!(
            !instance.instance_id.is_empty()
                && instance.instance_id.len() <= 256
                && instance
                    .instance_id
                    .bytes()
                    .all(|byte| (b' '..=b'~').contains(&byte)),
            "invalid controlled instance identity"
        );
        let root = self
            .config
            .runtime_root
            .join(format!("{}@main", descriptor.plugin_name));
        let expected = root
            .join("revisions")
            .join(runtime.spec.revision.to_string());
        ensure!(
            runtime.spec.revision_dir == expected,
            "runtime revision directory differs from controlled identity"
        );
        ordinary_ancestors(&root, &expected).await?;
        ensure!(
            tokio::fs::read_link(root.join("current")).await? == expected,
            "current configuration link does not select the applied revision"
        );
        let install = self.config.install_root.join(&descriptor.plugin_name);
        let binary_directory = runtime
            .spec
            .binary_path
            .parent()
            .context("runtime binary has no parent")?;
        ordinary_ancestors(&install, binary_directory).await?;
        ensure!(
            tokio::fs::read_link(install.join("current")).await? == binary_directory,
            "current executable link does not select the signed runtime"
        );
        ensure!(
            tokio::fs::symlink_metadata(&runtime.spec.binary_path)
                .await?
                .is_file(),
            "runtime executable is not ordinary"
        );
        ensure!(
            instance.binary_path == tokio::fs::canonicalize(&runtime.spec.binary_path).await?,
            "controlled instance is executing another binary"
        );
        ensure!(
            runtime.spec.files.len() <= MAX_CONFIG_FILES,
            "runtime has too many configuration files"
        );
        let mut total = 0usize;
        for (name, contents) in &runtime.spec.files {
            total = total
                .checked_add(name.len())
                .and_then(|value| value.checked_add(contents.len()))
                .context("configuration size overflow")?;
            ensure!(
                total <= MAX_CONFIG_BYTES,
                "configuration bundle exceeds inspection byte budget"
            );
        }
        let bundle = Bundle {
            files: runtime.spec.files.clone(),
        };
        crate::artifacts::validate_bundle_files(&bundle)?;
        let mut digest = ConfigurationDigest {
            digest: Sha256::new(),
            bytes: 0,
        };
        serde_json::to_writer(&mut digest, &bundle)?;
        ensure!(
            format!("{:x}", digest.digest.finalize()) == runtime.spec.config_hash,
            "configuration bundle digest differs from applied deployment"
        );
        let current = root.join("current");
        let relative = instance
            .config_path
            .strip_prefix(&current)
            .or_else(|_| instance.config_path.strip_prefix(&expected))
            .context("controlled instance configuration is outside the applied revision")?;
        let name = relative
            .to_str()
            .context("configuration path is not UTF-8")?;
        ensure!(
            bundle.files.contains_key(name),
            "controlled instance uses an undeclared configuration file"
        );
        let actual = configuration_inventory(&expected).await?;
        ensure!(
            actual == bundle.files.keys().cloned().collect(),
            "revision file inventory differs from the deployment"
        );
        for (name, contents) in &bundle.files {
            let path = expected.join(name);
            ordinary_ancestors(
                &expected,
                path.parent().context("configuration file has no parent")?,
            )
            .await?;
            let bytes = ordinary_file(&path, contents.len()).await?;
            ensure!(
                bytes == contents.as_bytes(),
                "configuration file differs from the deployment"
            );
        }
        ensure!(
            tokio::fs::canonicalize(&instance.config_path).await?
                == tokio::fs::canonicalize(expected.join(name)).await?,
            "controlled instance configuration link escaped the applied revision"
        );
        Ok(())
    }
}

struct ConfigurationDigest {
    digest: Sha256,
    bytes: usize,
}
impl std::io::Write for ConfigurationDigest {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let size = self
            .bytes
            .checked_add(bytes.len())
            .filter(|size| *size <= MAX_CONFIG_BYTES)
            .ok_or_else(|| {
                std::io::Error::other("configuration bundle exceeds inspection byte budget")
            })?;
        self.digest.update(bytes);
        self.bytes = size;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn ordinary_ancestors(root: &Path, child: &Path) -> Result<()> {
    let relative = child
        .strip_prefix(root)
        .context("path is outside controlled root")?;
    let mut path = root.to_path_buf();
    ensure!(
        tokio::fs::symlink_metadata(&path).await?.is_dir(),
        "controlled directory is not ordinary"
    );
    for part in relative.components() {
        ensure!(
            matches!(part, std::path::Component::Normal(_)),
            "unsafe controlled path"
        );
        path.push(part);
        ensure!(
            tokio::fs::symlink_metadata(&path).await?.is_dir(),
            "configuration ancestor is not ordinary"
        );
    }
    Ok(())
}

async fn configuration_inventory(root: &Path) -> Result<BTreeSet<String>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = BTreeSet::new();
    let mut entries = 0;
    while let Some(directory) = pending.pop() {
        let mut reader = tokio::fs::read_dir(&directory).await?;
        while let Some(entry) = reader.next_entry().await? {
            entries += 1;
            ensure!(
                entries <= MAX_CONFIG_FILES * 2,
                "configuration directory inventory exceeds budget"
            );
            let path = entry.path();
            let kind = entry.file_type().await?;
            if kind.is_dir() {
                pending.push(path);
            } else {
                ensure!(kind.is_file(), "configuration contains a non-ordinary file");
                files.insert(
                    path.strip_prefix(root)?
                        .to_str()
                        .context("configuration path is not UTF-8")?
                        .to_owned(),
                );
            }
        }
    }
    Ok(files)
}

async fn ordinary_file(path: &Path, expected_size: usize) -> Result<Vec<u8>> {
    ensure!(
        expected_size <= MAX_CONFIG_BYTES,
        "configuration file exceeds byte budget"
    );
    let before = tokio::fs::symlink_metadata(path).await?;
    ensure!(
        before.is_file() && before.len() == expected_size as u64,
        "configuration file type or size differs"
    );
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    // Nonblocking prevents a substituted FIFO from hanging before the descriptor check.
    #[cfg(target_os = "linux")]
    options.custom_flags(0x20000 | 0x800);
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    options.custom_flags(0x100 | 0x4);
    let file = options.open(path).await?;
    let opened = file.metadata().await?;
    ensure!(
        opened.is_file() && opened.len() == before.len(),
        "configuration file changed before inspection"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            before.dev() == opened.dev() && before.ino() == opened.ino(),
            "configuration inode changed"
        );
    }
    let mut bytes = Vec::with_capacity(expected_size);
    file.take(expected_size as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    ensure!(
        bytes.len() == expected_size,
        "configuration size changed during inspection"
    );
    let after = tokio::fs::symlink_metadata(path).await?;
    ensure!(
        after.is_file() && after.len() == opened.len(),
        "configuration path changed during inspection"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            after.dev() == opened.dev() && after.ino() == opened.ino(),
            "configuration path inode changed"
        );
    }
    Ok(bytes)
}

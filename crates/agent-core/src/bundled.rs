use crate::artifacts::{ensure_ordinary_directory_if_present, safe_component};
use anyhow::{Context, Result, bail, ensure};
use apsp_adapter_sdk::Privileged;
pub use apsp_protocol::bundled::{
    BUNDLED_MARKER, BundledFileRecord, BundledManifest, EmbeddedAgentManifest,
    EmbeddedComponentRecord, MAX_BUNDLED_FILES, MAX_BUNDLED_MARKER_BYTES, MAX_BUNDLED_PATH_DEPTH,
    MAX_BUNDLED_UNPACKED_BYTES, encode_embedded_agent_manifest_frame,
    expected_embedded_component_digest, extract_embedded_agent_manifest,
    extract_embedded_agent_manifest_optional, normalize_bundled_relative_path,
    validate_bundled_component_contents,
};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub enum BundledPayload {
    /// Compressed `.tar.gz` archive containing `binary_name` (and optional auxiliary files).
    TarGz {
        archive_sha256: String,
        bytes: Cow<'static, [u8]>,
    },
    /// Executed via the running verified Agent binary (`<current_exe> <subcommand> ...`).
    AgentSubcommand { subcommand: String },
}

#[derive(Clone, Debug)]
pub struct BundledItem {
    pub component_name: String,
    pub binary_name: String,
    pub version: String,
    pub payload: BundledPayload,
}

#[derive(Clone, Debug, Default)]
pub struct BundledComponents {
    /// Keyed by runtime adapter `module` (e.g. the proxy runtime module id).
    pub runtimes: BTreeMap<String, BundledItem>,
    /// Keyed by diagnostic adapter component name (e.g. `"tcpquality"`, `"ipquality"`, `"nodequality"`).
    pub tools: BTreeMap<String, BundledItem>,
}

impl BundledComponents {
    pub fn is_empty(&self) -> bool {
        self.runtimes.is_empty() && self.tools.is_empty()
    }
}

fn normalized_sha256(value: &str) -> Result<String> {
    ensure!(
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid bundled SHA256 digest"
    );
    Ok(value.to_ascii_lowercase())
}

/// Returns the deterministic bundle digest for materializable payloads without decompressing `.tar.gz`.
pub(crate) fn bundle_digest(item: &BundledItem) -> Result<Option<String>> {
    ensure!(
        safe_component(&item.component_name)
            && safe_component(&item.binary_name)
            && apsp_protocol::bundled::valid_version_segment(&item.version),
        "invalid bundled component identity"
    );
    match &item.payload {
        BundledPayload::TarGz {
            archive_sha256,
            bytes,
        } => {
            ensure!(
                bytes.len() >= 18 && bytes[0] == 0x1f && bytes[1] == 0x8b,
                "bundled archive is not a valid gzip stream"
            );
            Ok(Some(normalized_sha256(archive_sha256)?))
        }
        BundledPayload::AgentSubcommand { subcommand } => {
            ensure!(
                safe_component(subcommand),
                "invalid bundled Agent subcommand"
            );
            Ok(None)
        }
    }
}

/// Lightweight startup check: verifies descriptor syntax and `AgentSubcommand` executable
/// without decompressing any embedded `.tar.gz` archive on monitoring-only hosts.
pub(crate) fn validate_components(components: &BundledComponents) -> Result<()> {
    for (module, item) in &components.runtimes {
        ensure!(
            safe_component(module),
            "invalid bundled runtime module name"
        );
        ensure!(
            bundle_digest(item)?.is_some(),
            "bundled runtime cannot use an Agent subcommand payload"
        );
    }
    for (tool, item) in &components.tools {
        ensure!(
            safe_component(tool) && tool == &item.component_name,
            "bundled diagnostic tool key does not match its component name"
        );
        if bundle_digest(item)?.is_none() {
            let _ = current_agent_binary()?;
        }
    }
    Ok(())
}

pub(crate) fn current_agent_binary() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("resolve running Agent executable")?;
    let canonical = std::fs::canonicalize(&exe).context("canonicalize running Agent executable")?;
    ensure!(
        canonical.is_absolute()
            && canonical.components().all(|part| matches!(
                part,
                Component::RootDir | Component::Normal(_) | Component::Prefix(_)
            )),
        "running Agent executable path is invalid"
    );
    let metadata = std::fs::symlink_metadata(&canonical)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "running Agent executable must be an ordinary file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = metadata.permissions().mode();
        ensure!(
            mode & 0o002 == 0,
            "running Agent executable cannot be world-writable"
        );
        if metadata.uid() == 0 {
            ensure!(
                mode & 0o022 == 0,
                "root-owned Agent executable cannot be group-writable"
            );
        }
    }
    Ok(canonical)
}

fn current_agent_sha256_sync() -> Result<String> {
    type Stamp = (PathBuf, u64, Option<(u64, u64, i64, i64, i64, i64)>);
    static MEMO: std::sync::OnceLock<std::sync::Mutex<Option<(Stamp, String)>>> =
        std::sync::OnceLock::new();

    let path = current_agent_binary()?;
    let metadata = std::fs::symlink_metadata(&path)?;
    let size = metadata.len();
    #[cfg(unix)]
    let (stamp, settled) = {
        use std::os::unix::fs::MetadataExt;
        let tuple = (
            metadata.dev(),
            metadata.ino(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        );
        let settled = u64::try_from(tuple.4)
            .ok()
            .zip(u32::try_from(tuple.5).ok())
            .and_then(|(s, ns)| std::time::UNIX_EPOCH.checked_add(std::time::Duration::new(s, ns)))
            .and_then(|changed| std::time::SystemTime::now().duration_since(changed).ok())
            .is_some_and(|age| age >= std::time::Duration::from_secs(2));
        (Some(tuple), settled)
    };
    #[cfg(not(unix))]
    let (stamp, settled): (Option<(u64, u64, i64, i64, i64, i64)>, bool) = (None, false);

    let key = (path.clone(), size, stamp);
    if settled
        && let Some(digest) = MEMO
            .get_or_init(Default::default)
            .lock()
            .ok()
            .and_then(|guard| {
                guard
                    .as_ref()
                    .filter(|(k, _)| k == &key)
                    .map(|(_, v)| v.clone())
            })
    {
        return Ok(digest);
    }

    let mut file = std::fs::File::open(&path)
        .with_context(|| format!("open running Agent executable {}", path.display()))?;
    let rustls::SupportedCipherSuite::Tls13(suite) =
        rustls::crypto::ring::cipher_suite::TLS13_AES_128_GCM_SHA256
    else {
        bail!("TLS13_AES_128_GCM_SHA256 must be a TLS 1.3 cipher suite");
    };
    let mut hash = suite.common.hash_provider.start();
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let digest_bytes = hash.finish();
    let mut digest = String::with_capacity(64);
    for byte in digest_bytes.as_ref() {
        use std::fmt::Write as _;
        let _ = write!(&mut digest, "{byte:02x}");
    }
    if settled && let Ok(mut guard) = MEMO.get_or_init(Default::default).lock() {
        *guard = Some((key, digest.clone()));
    }
    Ok(digest)
}

pub(crate) async fn current_agent_sha256() -> Result<String> {
    tokio::task::spawn_blocking(current_agent_sha256_sync).await?
}

pub(crate) fn is_bundled_directory_sync(directory: &Path) -> bool {
    let Ok(dir_meta) = std::fs::symlink_metadata(directory) else {
        return false;
    };
    if !dir_meta.is_dir() || dir_meta.file_type().is_symlink() {
        return false;
    }
    let Some(dir_name) = directory.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let marker = directory.join(BUNDLED_MARKER);
    let Ok(file_meta) = std::fs::symlink_metadata(&marker) else {
        return false;
    };
    if !file_meta.is_file()
        || file_meta.file_type().is_symlink()
        || file_meta.len() > MAX_BUNDLED_MARKER_BYTES as u64
    {
        return false;
    }
    let Ok(bytes) = std::fs::read(&marker) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_slice::<BundledManifest>(&bytes) else {
        return false;
    };
    manifest.validate().is_ok() && matches_version_directory_name(dir_name, &manifest.version)
}

pub(crate) fn matches_version_directory_name(dir_name: &str, version: &str) -> bool {
    if dir_name == version {
        return true;
    }
    dir_name
        .strip_prefix(version)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|suffix| {
            suffix.len() == 12
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

pub(crate) async fn read_bundled_manifest(directory: &Path) -> Result<Option<BundledManifest>> {
    ensure_ordinary_directory_if_present(directory).await?;
    if let Some(parent) = directory.parent() {
        ensure_ordinary_directory_if_present(parent).await?;
    }
    let path = directory.join(BUNDLED_MARKER);
    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() <= MAX_BUNDLED_MARKER_BYTES as u64,
        "bundled manifest must be a bounded ordinary file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = metadata.permissions().mode();
        ensure!(
            mode & 0o002 == 0 && (metadata.uid() != 0 || mode & 0o022 == 0),
            "bundled manifest has writable permissions"
        );
    }
    let bytes = tokio::fs::read(&path).await?;
    ensure!(
        bytes.len() <= MAX_BUNDLED_MARKER_BYTES,
        "bundled manifest exceeds size limit"
    );
    let manifest: BundledManifest =
        serde_json::from_slice(&bytes).context("invalid bundled manifest JSON")?;
    manifest.validate().map_err(anyhow::Error::msg)?;
    let dir_name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid bundled version directory")?;
    ensure!(
        matches_version_directory_name(dir_name, &manifest.version),
        "bundled directory name does not match its manifest version"
    );
    Ok(Some(manifest))
}

pub(crate) async fn verify_manifest_files(
    directory: &Path,
    manifest: &BundledManifest,
) -> Result<()> {
    manifest.validate().map_err(anyhow::Error::msg)?;
    ensure_ordinary_directory_if_present(directory).await?;
    for (name, record) in &manifest.files {
        let relative = Path::new(name);
        let mut cursor = directory.to_path_buf();
        if let Some(parent_rel) = relative.parent() {
            for part in parent_rel.components() {
                cursor.push(part);
                ensure_ordinary_directory_if_present(&cursor).await?;
            }
        }
        let file_path = directory.join(relative);
        crate::artifacts::verify_file_digest(&file_path, record.size, &record.sha256).await?;
        #[cfg(unix)]
        if record.executable {
            use std::os::unix::fs::PermissionsExt;
            let mode = tokio::fs::symlink_metadata(&file_path)
                .await?
                .permissions()
                .mode();
            ensure!(
                mode & 0o111 != 0 && mode & 0o002 == 0,
                "bundled executable permissions are invalid"
            );
        }
    }
    Ok(())
}

/// Verifies an on-disk binary against the compiled `BundledItem` identity without decompressing
/// the archive when `.bundled.json` already matches `bundle_sha256`.
pub async fn verify_compiled_item(
    item: &BundledItem,
    install_root: &Path,
    binary_path: &Path,
) -> Result<BundledManifest> {
    let expected_digest = bundle_digest(item)?
        .context("Agent subcommand payload is not stored in a component directory")?;
    let directory = binary_path
        .parent()
        .context("bundled binary has no version directory")?;
    let component_dir = directory
        .parent()
        .context("bundled binary has no component directory")?;
    let dir_name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid bundled binary directory")?;
    ensure!(
        binary_path.is_absolute()
            && component_dir == install_root.join(&item.component_name)
            && matches_version_directory_name(dir_name, &item.version)
            && binary_path.file_name().and_then(|name| name.to_str())
                == Some(item.binary_name.as_str()),
        "bundled binary path differs from installation identity"
    );
    let saved = read_bundled_manifest(directory)
        .await?
        .context("bundled directory has no manifest")?;
    ensure!(
        saved.component_name == item.component_name
            && saved.binary_name == item.binary_name
            && saved.version == item.version
            && saved.bundle_sha256 == expected_digest,
        "installed bundled manifest differs from compiled component"
    );
    verify_manifest_files(directory, &saved).await?;
    Ok(saved)
}

/// Checks whether `<install_root>/<component_name>/current` selects this exact compiled `BundledItem`.
pub(crate) async fn current_matches_compiled(
    item: &BundledItem,
    install_root: &Path,
) -> Result<Option<BundledManifest>> {
    let Some(expected_digest) = bundle_digest(item)? else {
        return Ok(None);
    };
    let component_dir = install_root.join(&item.component_name);
    let current = component_dir.join("current");
    let Ok(target) = crate::system::deploy::reference(&current) else {
        return Ok(None);
    };
    let Ok(resolved) = tokio::fs::canonicalize(&target).await else {
        return Ok(None);
    };
    let Ok(canonical_component) = tokio::fs::canonicalize(&component_dir).await else {
        return Ok(None);
    };
    if resolved.parent() != Some(canonical_component.as_path()) {
        return Ok(None);
    }
    let Ok(Some(saved)) = read_bundled_manifest(&resolved).await else {
        return Ok(None);
    };
    if saved.component_name != item.component_name
        || saved.binary_name != item.binary_name
        || saved.version != item.version
        || saved.bundle_sha256 != expected_digest
    {
        return Ok(None);
    }
    if verify_manifest_files(&resolved, &saved).await.is_err() {
        return Ok(None);
    }
    Ok(Some(saved))
}

/// Materializes `item` on demand into `<install_root>/<component_name>/<version>/` (or
/// `<version>-<hash12>` when `<version>` already holds a different payload so rollback remains
/// intact) and returns the verified executable path.
pub async fn ensure_materialized(
    item: &BundledItem,
    install_root: &Path,
    privileged: &dyn Privileged,
) -> Result<PathBuf> {
    ensure!(
        install_root.is_absolute()
            && install_root.components().all(|part| matches!(
                part,
                Component::RootDir | Component::Normal(_) | Component::Prefix(_)
            )),
        "invalid bundled installation root"
    );
    let expected_digest = bundle_digest(item)?
        .context("Agent subcommand payload cannot be materialized into install_root")?;
    ensure_ordinary_directory_if_present(install_root).await?;
    let component_dir = install_root.join(&item.component_name);
    ensure_ordinary_directory_if_present(&component_dir).await?;
    privileged.create_dir(install_root, 0o755, None).await?;
    privileged.create_dir(&component_dir, 0o755, None).await?;

    let primary_dir = component_dir.join(&item.version);
    let destination = match tokio::fs::symlink_metadata(&primary_dir).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => primary_dir,
        Ok(metadata) => {
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "bundled version path must be an ordinary directory"
            );
            let candidate_bin = primary_dir.join(&item.binary_name);
            if verify_compiled_item(item, install_root, &candidate_bin)
                .await
                .is_ok()
            {
                return Ok(candidate_bin);
            }
            let alt_dir =
                component_dir.join(format!("{}-{}", item.version, &expected_digest[..12]));
            let alt_bin = alt_dir.join(&item.binary_name);
            if tokio::fs::symlink_metadata(&alt_dir).await.is_ok() {
                if verify_compiled_item(item, install_root, &alt_bin)
                    .await
                    .is_ok()
                {
                    return Ok(alt_bin);
                }
                privileged.remove_path(&alt_dir).await?;
            }
            alt_dir
        }
        Err(error) => return Err(error.into()),
    };

    let staging = component_dir.join(format!(".bundled-{}", Uuid::new_v4()));
    privileged.create_dir(&staging, 0o700, None).await?;
    let staged: Result<()> = async {
        let manifest = match &item.payload {
            BundledPayload::TarGz {
                archive_sha256,
                bytes,
            } => stage_tar_gz(item, archive_sha256, bytes.as_ref(), &staging, privileged).await?,
            BundledPayload::AgentSubcommand { .. } => {
                bail!("Agent subcommand payload cannot be materialized")
            }
        };
        let manifest_bytes = serde_json::to_vec(&manifest)?;
        privileged
            .write_file(&staging.join(BUNDLED_MARKER), &manifest_bytes, 0o644, None)
            .await?;
        privileged.create_dir(&staging, 0o755, None).await?;
        privileged.publish_directory(&staging, &destination).await?;
        Ok(())
    }
    .await;
    if let Err(error) = staged {
        let _ = privileged.remove_path(&staging).await;
        let published_bin = destination.join(&item.binary_name);
        if verify_compiled_item(item, install_root, &published_bin)
            .await
            .is_ok()
        {
            return Ok(published_bin);
        }
        return Err(error);
    }

    let binary_path = destination.join(&item.binary_name);
    verify_compiled_item(item, install_root, &binary_path).await?;
    Ok(binary_path)
}

async fn ensure_staging_parents(
    staging: &Path,
    relative: &str,
    created: &mut BTreeSet<PathBuf>,
    privileged: &dyn Privileged,
) -> Result<()> {
    if let Some(parent) = Path::new(relative).parent() {
        let mut cursor = staging.to_path_buf();
        for part in parent.components() {
            cursor.push(part);
            if created.insert(cursor.clone()) {
                privileged.create_dir(&cursor, 0o755, None).await?;
            }
        }
    }
    Ok(())
}

async fn stage_tar_gz(
    item: &BundledItem,
    archive_sha256: &str,
    bytes: &[u8],
    staging: &Path,
    privileged: &dyn Privileged,
) -> Result<BundledManifest> {
    let expected_archive = normalized_sha256(archive_sha256)?;
    let actual_archive = format!("{:x}", Sha256::digest(bytes));
    ensure!(
        actual_archive == expected_archive,
        "bundled archive SHA256 does not match compiled digest"
    );

    let extracted = {
        let decoder = flate2::read::MultiGzDecoder::new(bytes).take(MAX_BUNDLED_UNPACKED_BYTES + 1);
        let mut archive = tar::Archive::new(decoder);
        let mut items = Vec::new();
        let mut seen = BTreeSet::new();
        let mut total = 0_u64;
        for entry in archive.entries()?.raw(true) {
            let mut entry = entry?;
            let entry_type = entry.header().entry_type();
            let raw_bytes = entry.path_bytes();
            let raw_str = std::str::from_utf8(raw_bytes.as_ref())
                .context("bundled archive path is not UTF-8")?;
            let trimmed = raw_str.trim_end_matches('/');
            if entry_type.is_dir() {
                if !trimmed.is_empty() {
                    let _ = normalize_bundled_relative_path(Path::new(trimmed))
                        .map_err(anyhow::Error::msg)?;
                }
                continue;
            }
            ensure!(
                entry_type.is_file(),
                "bundled archive contains a non-ordinary entry"
            );
            let name =
                normalize_bundled_relative_path(Path::new(raw_str)).map_err(anyhow::Error::msg)?;
            ensure!(
                seen.insert(name.clone()),
                "bundled archive contains duplicate entry"
            );
            ensure!(
                seen.len() <= MAX_BUNDLED_FILES,
                "bundled archive exceeds maximum file count"
            );
            let size = entry.size();
            total = total
                .checked_add(size)
                .filter(|sum| *sum <= MAX_BUNDLED_UNPACKED_BYTES)
                .context("bundled archive exceeds unpacked size limit")?;
            let header_mode = entry.header().mode().unwrap_or(0o644);
            let executable = name == item.binary_name || (header_mode & 0o111) != 0;
            let mut content = Vec::with_capacity(usize::try_from(size)?);
            entry.read_to_end(&mut content)?;
            ensure!(
                content.len() as u64 == size,
                "bundled archive entry length mismatch"
            );
            let sha256 = format!("{:x}", Sha256::digest(&content));
            items.push((name, content, size, sha256, executable));
        }
        let mut decoder = archive.into_inner();
        let mut tail = [0_u8; 8192];
        loop {
            let count = decoder.read(&mut tail)?;
            if count == 0 {
                break;
            }
            ensure!(
                tail[..count].iter().all(|byte| *byte == 0),
                "bundled archive contains trailing data"
            );
        }
        ensure!(
            decoder.limit() > 0,
            "bundled archive exceeds unpacked size limit"
        );
        items
    };

    let mut records = BTreeMap::new();
    let mut created_dirs = BTreeSet::new();
    for (name, content, size, sha256, executable) in extracted {
        ensure_staging_parents(staging, &name, &mut created_dirs, privileged).await?;
        privileged
            .write_file(
                &staging.join(&name),
                &content,
                if executable { 0o755 } else { 0o644 },
                None,
            )
            .await?;
        records.insert(
            name,
            BundledFileRecord {
                sha256,
                size,
                executable,
            },
        );
    }
    let binary = records
        .get(&item.binary_name)
        .context("bundled archive is missing the primary binary")?;
    let manifest = BundledManifest {
        schema: 1,
        component_name: item.component_name.clone(),
        binary_name: item.binary_name.clone(),
        version: item.version.clone(),
        bundle_sha256: expected_archive,
        binary_sha256: binary.sha256.clone(),
        files: records,
    };
    manifest.validate().map_err(anyhow::Error::msg)?;
    Ok(manifest)
}

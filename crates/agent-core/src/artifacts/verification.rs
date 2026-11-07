use super::{MARKER, ensure_ordinary_directory_if_present};
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::Descriptor;
use apsp_protocol::release::{
    MAX_CHECKSUMS_BYTES, MAX_METADATA_BYTES, MAX_SIGNATURE_BYTES, ReleaseProof, TrustedKeys,
    VerifiedArtifact, VerifiedRelease, verify_release,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::io::AsyncReadExt;

pub(super) const MAX_PROOF: usize =
    2 * (MAX_METADATA_BYTES + MAX_CHECKSUMS_BYTES + MAX_SIGNATURE_BYTES);

pub(super) async fn ordinary_bytes(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    ensure!(
        metadata.is_file() && metadata.len() <= maximum as u64,
        "proof must be an ordinary bounded file"
    );
    let mut bytes = Vec::new();
    tokio::fs::File::open(path)
        .await?
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    ensure!(bytes.len() <= maximum, "proof exceeds size limit");
    Ok(bytes)
}

pub(super) async fn read_proof(directory: &Path) -> Result<ReleaseProof> {
    ensure_ordinary_directory_if_present(directory).await?;
    ensure_ordinary_directory_if_present(
        directory
            .parent()
            .context("artifact directory has no parent")?,
    )
    .await?;
    let metadata = directory.join("release.json");
    if tokio::fs::symlink_metadata(&metadata).await.is_ok() {
        return Ok(ReleaseProof {
            metadata_json: String::from_utf8(ordinary_bytes(&metadata, MAX_METADATA_BYTES).await?)?,
            checksums: String::from_utf8(
                ordinary_bytes(&directory.join("SHA256SUMS"), MAX_CHECKSUMS_BYTES).await?,
            )?,
            signature: String::from_utf8(
                ordinary_bytes(&directory.join("SHA256SUMS.minisig"), MAX_SIGNATURE_BYTES).await?,
            )?,
        });
    }
    let marker: super::InstalledArtifact = serde_json::from_slice(
        &ordinary_bytes(&directory.join(MARKER), MAX_PROOF)
            .await
            .context("installed artifact has no signed proof")?,
    )?;
    marker
        .proof
        .context("installed artifact has no signed proof")
}

/// Identifies a file's content without reading it: device, inode, and the
/// modification and change times. Any write or replacement moves the change
/// time, which userspace cannot set back.
type FileStamp = (u64, u64, i64, i64, i64, i64);

#[cfg(unix)]
fn file_stamp(metadata: &std::fs::Metadata) -> Option<FileStamp> {
    use std::os::unix::fs::MetadataExt;
    Some((
        metadata.dev(),
        metadata.ino(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    ))
}

#[cfg(not(unix))]
fn file_stamp(_: &std::fs::Metadata) -> Option<FileStamp> {
    None
}

/// Unchanged files are hashed again at least this often, which also catches
/// corruption below the filesystem.
const REVERIFY_AFTER: Duration = Duration::from_secs(60 * 60);
/// File times have coarse granularity. A file changed this recently could be
/// changed again without its stamp moving, so its result is not remembered.
const SETTLED_AFTER: Duration = Duration::from_secs(2);

/// Whether the stamp's change time is safely before `started`.
fn settled(stamp: &FileStamp, started: SystemTime) -> bool {
    let (seconds, nanoseconds) = (stamp.4, stamp.5);
    let (Ok(seconds), Ok(nanoseconds)) = (u64::try_from(seconds), u32::try_from(nanoseconds))
    else {
        return false;
    };
    UNIX_EPOCH
        .checked_add(Duration::new(seconds, nanoseconds))
        .and_then(|changed| started.duration_since(changed).ok())
        .is_some_and(|age| age >= SETTLED_AFTER)
}

struct Verified {
    stamp: FileStamp,
    size: u64,
    sha256: String,
    at: Instant,
}

/// Files whose digest matched, so steady-state checks (several a minute for
/// the runtime binary) do not re-read tens of megabytes each time.
fn verified_files() -> &'static Mutex<HashMap<PathBuf, Verified>> {
    static VERIFIED: OnceLock<Mutex<HashMap<PathBuf, Verified>>> = OnceLock::new();
    VERIFIED.get_or_init(Mutex::default)
}

async fn verify_digest(path: &Path, expected_size: u64, expected_sha256: &str) -> Result<()> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    ensure!(
        metadata.is_file() && metadata.len() == expected_size,
        "installed artifact is not an ordinary file of its signed size"
    );
    let stamp = file_stamp(&metadata);
    if let Some(stamp) = stamp {
        let verified = verified_files().lock().ok().is_some_and(|files| {
            files.get(path).is_some_and(|known| {
                known.stamp == stamp
                    && known.size == expected_size
                    && known.sha256 == expected_sha256
                    && known.at.elapsed() < REVERIFY_AFTER
            })
        });
        if verified {
            return Ok(());
        }
    }
    let started = SystemTime::now();
    let result = hash_matches(path, expected_size, expected_sha256).await;
    if let Ok(mut files) = verified_files().lock() {
        match (&result, stamp) {
            (Ok(()), Some(stamp)) if settled(&stamp, started) => {
                files.insert(
                    path.to_owned(),
                    Verified {
                        stamp,
                        size: expected_size,
                        sha256: expected_sha256.to_owned(),
                        at: Instant::now(),
                    },
                );
            }
            _ => {
                files.remove(path);
            }
        }
    }
    result
}

async fn hash_matches(path: &Path, expected_size: u64, expected_sha256: &str) -> Result<()> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hash = Sha256::new();
    // Large reads: each one is a hop to the blocking pool.
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut length = 0_u64;
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        length += count as u64;
        ensure!(
            length <= expected_size,
            "installed artifact grew beyond signed size"
        );
        hash.update(&buffer[..count]);
    }
    ensure!(
        length == expected_size && format!("{:x}", hash.finalize()) == expected_sha256,
        "installed artifact file differs from signed release"
    );
    Ok(())
}

pub(super) async fn verify_file(binary: &Path, verified: &VerifiedArtifact) -> Result<()> {
    verify_digest(
        binary,
        verified.metadata().binary_size,
        &verified.metadata().binary_sha256,
    )
    .await?;
    let directory = binary.parent().context("binary has no parent")?;
    for (name, expected) in &verified.metadata().auxiliary_files {
        verify_digest(&directory.join(name), expected.size, &expected.sha256)
            .await
            .context("auxiliary artifact differs from signed release")?;
    }
    Ok(())
}

pub(super) fn signed_release(proof: &ReleaseProof, keys: &TrustedKeys) -> Result<VerifiedRelease> {
    let release = verify_release(proof, keys)?;
    ensure!(
        release.metadata().protocol_min <= apsp_protocol::PROTOCOL_MAX
            && release.metadata().protocol_max >= apsp_protocol::PROTOCOL_MIN,
        "signed release protocol range is incompatible with this Agent"
    );
    Ok(release)
}

pub(super) fn signed_artifact(
    proof: &ReleaseProof,
    descriptor: &Descriptor,
    version: &str,
    keys: &TrustedKeys,
) -> Result<VerifiedArtifact> {
    let release = signed_release(proof, keys)?;
    let verified = crate::runtime_platform::artifact(&release, &descriptor.plugin_name, version)?;
    ensure!(
        verified.metadata().binary_name == descriptor.binary_name
            && verified.metadata().format == "tar.gz"
            && verified
                .metadata()
                .auxiliary_files
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                == descriptor
                    .auxiliary_files
                    .iter()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>(),
        "signed artifact format or executable name differs from adapter"
    );
    Ok(verified)
}

pub(crate) fn runtime_descriptor(
    adapter: &dyn apsp_adapter_sdk::Adapter,
    version: &str,
    proof: &ReleaseProof,
    keys: &TrustedKeys,
) -> Result<Descriptor> {
    let release = signed_release(proof, keys)?;
    let verified =
        crate::runtime_platform::artifact(&release, &adapter.describe().plugin_name, version)?;
    Ok(adapter.describe_artifact(version, &verified.metadata().arch))
}

pub(crate) async fn installed_descriptor(
    adapter: &dyn apsp_adapter_sdk::Adapter,
    version: &str,
    binary: &Path,
    keys: &TrustedKeys,
) -> Result<Descriptor> {
    let proof = read_proof(
        binary
            .parent()
            .context("artifact binary has no directory")?,
    )
    .await?;
    runtime_descriptor(adapter, version, &proof, keys)
}

pub(crate) async fn verify_expected(
    binary: &Path,
    descriptor: &Descriptor,
    version: &str,
    keys: &TrustedKeys,
) -> Result<()> {
    let directory = binary
        .parent()
        .context("artifact binary has no directory")?;
    ensure!(
        directory.file_name().and_then(|name| name.to_str()) == Some(version)
            && binary.file_name().and_then(|name| name.to_str())
                == Some(descriptor.binary_name.as_str()),
        "installed artifact path differs from requested identity"
    );
    let proof = read_proof(directory).await?;
    let verified = signed_artifact(&proof, descriptor, version, keys)?;
    verify_file(binary, &verified).await
}

pub(super) async fn verify_binary_with_keys(
    binary: &Path,
    expected_name: &str,
    expected_format: &str,
    keys: &TrustedKeys,
) -> Result<()> {
    ensure!(
        binary.is_absolute(),
        "artifact verification requires an absolute path"
    );
    let directory = binary.parent().context("binary has no parent")?;
    let directory_metadata = tokio::fs::symlink_metadata(directory).await?;
    let resolved = if directory_metadata.file_type().is_symlink() {
        ensure!(
            directory.file_name().is_some_and(|name| name == "current"),
            "uncontrolled artifact directory symlink"
        );
        let resolved = tokio::fs::canonicalize(directory).await?;
        ensure!(
            resolved.parent()
                == Some(
                    tokio::fs::canonicalize(directory.parent().context("current has no parent")?)
                        .await?
                        .as_path()
                ),
            "current artifact link escapes its installation directory"
        );
        resolved
    } else {
        ensure!(
            directory_metadata.is_dir(),
            "artifact version is not an ordinary directory"
        );
        directory.to_path_buf()
    };
    let version = resolved
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid artifact version path")?;
    let name = binary
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid binary path")?;
    let proof = read_proof(&resolved).await?;
    let release = signed_release(&proof, keys)?;
    let verified = crate::runtime_platform::artifact(&release, expected_name, version)?;
    ensure!(
        verified.metadata().binary_name == name && verified.metadata().format == expected_format,
        "installed artifact role, format, or executable name differs from requested identity"
    );
    verify_file(&resolved.join(name), &verified).await
}

/// Verifies a cached executable offline without reading device identity or configuration.
pub async fn verify_installed_binary(
    binary: &Path,
    expected_name: &str,
    expected_format: &str,
) -> Result<()> {
    verify_binary_with_keys(
        binary,
        expected_name,
        expected_format,
        &TrustedKeys::compiled()?,
    )
    .await
}

/// Checks a release proof using this Agent's compiled trust roots, without network access.
pub async fn verify_release_directory(directory: &Path) -> Result<()> {
    ensure_ordinary_directory_if_present(directory).await?;
    ensure_ordinary_directory_if_present(
        directory
            .parent()
            .context("release directory has no parent")?,
    )
    .await?;
    let proof = ReleaseProof {
        metadata_json: String::from_utf8(
            ordinary_bytes(&directory.join("release.json"), MAX_METADATA_BYTES).await?,
        )?,
        checksums: String::from_utf8(
            ordinary_bytes(&directory.join("SHA256SUMS"), MAX_CHECKSUMS_BYTES).await?,
        )?,
        signature: String::from_utf8(
            ordinary_bytes(&directory.join("SHA256SUMS.minisig"), MAX_SIGNATURE_BYTES).await?,
        )?,
    };
    signed_release(&proof, &TrustedKeys::compiled()?)?;
    Ok(())
}

#[cfg(test)]
mod digest_tests {
    use super::*;

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[tokio::test]
    async fn a_freshly_changed_file_is_hashed_on_every_check() -> Result<()> {
        let path = std::env::temp_dir().join(format!("apsp-digest-memo-{}", uuid::Uuid::new_v4()));
        let original = b"TEST_ONLY original payload".to_vec();
        tokio::fs::write(&path, &original).await?;
        verify_digest(&path, original.len() as u64, &sha256(&original)).await?;
        // Same size, rewritten in place within the timestamp granularity.
        let mut changed = original.clone();
        changed[0] ^= 1;
        tokio::fs::write(&path, &changed).await?;
        let result = verify_digest(&path, original.len() as u64, &sha256(&original)).await;
        tokio::fs::remove_file(&path).await?;
        assert!(
            result.is_err(),
            "a changed file must not pass on a remembered digest"
        );
        Ok(())
    }

    #[test]
    fn only_settled_change_times_are_remembered() {
        let now = SystemTime::now();
        let seconds = |at: SystemTime| {
            let since = at.duration_since(UNIX_EPOCH).unwrap();
            (since.as_secs() as i64, since.subsec_nanos() as i64)
        };
        let (old_s, old_ns) = seconds(now - Duration::from_secs(10));
        let (new_s, new_ns) = seconds(now - Duration::from_millis(500));
        assert!(settled(&(1, 2, old_s, old_ns, old_s, old_ns), now));
        assert!(!settled(&(1, 2, new_s, new_ns, new_s, new_ns), now));
        assert!(!settled(&(1, 2, -1, 0, -1, 0), now));
    }
}

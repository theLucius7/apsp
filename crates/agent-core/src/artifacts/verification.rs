use super::{MARKER, ensure_ordinary_directory_if_present};
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::Descriptor;
use apsp_protocol::release::{
    MAX_CHECKSUMS_BYTES, MAX_METADATA_BYTES, MAX_SIGNATURE_BYTES, ReleaseProof, TrustedKeys,
    VerifiedArtifact, VerifiedRelease, verify_release,
};
use sha2::{Digest, Sha256};
use std::path::Path;
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

async fn verify_digest(path: &Path, expected_size: u64, expected_sha256: &str) -> Result<()> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    ensure!(
        metadata.is_file() && metadata.len() == expected_size,
        "installed artifact is not an ordinary file of its signed size"
    );
    let mut file = tokio::fs::File::open(path).await?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
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

use base64::{Engine, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const RELEASE_SCHEMA: u32 = 1;
pub const RELEASE_SOURCE_REPO: &str = "theLucius7/apsp";
pub const ARTIFACT_SIGNATURE_CAPABILITY: &str = "artifact:minisign-v1";
pub const MAX_CHECKSUMS_BYTES: usize = 8 * 1024;
pub const MAX_METADATA_BYTES: usize = 32 * 1024;
pub const MAX_SIGNATURE_BYTES: usize = 16 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseProof {
    pub metadata_json: String,
    pub checksums: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMetadata {
    pub schema: u32,
    pub source_repo: String,
    pub tag: String,
    pub protocol_min: u16,
    pub protocol_max: u16,
    pub artifacts: Vec<ReleaseArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseArtifact {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub format: String,
    pub binary_name: String,
    pub archive_size: u64,
    pub binary_sha256: String,
    pub binary_size: u64,
    pub asset_name: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub auxiliary_files: BTreeMap<String, ReleaseFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseFile {
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Error)]
pub enum ReleaseError {
    #[error("release trust roots were not configured at build time")]
    MissingTrustRoot,
    #[error("invalid release trust roots")]
    InvalidTrustRoot,
    #[error("invalid or oversized release proof")]
    InvalidProof,
    #[error("release signature is not trusted")]
    UntrustedSignature,
    #[error("invalid release checksums")]
    InvalidChecksums,
    #[error("invalid release metadata")]
    InvalidMetadata,
    #[error("release does not contain the requested artifact")]
    MissingArtifact,
    #[error("artifact identity does not match its signed release")]
    IdentityMismatch,
    #[error("artifact bytes do not match their signed digest or size")]
    BytesMismatch,
}

#[derive(Clone)]
pub struct TrustedKeys {
    keys: Vec<PublicKey>,
}

impl TrustedKeys {
    pub fn compiled() -> Result<Self, ReleaseError> {
        Self::from_json(
            option_env!("APSP_RELEASE_PUBLIC_KEYS").ok_or(ReleaseError::MissingTrustRoot)?,
        )
    }

    /// Explicit trust is for callers that already possess a trusted key, including tests.
    pub fn from_json(value: &str) -> Result<Self, ReleaseError> {
        if value.len() > 32 * 1024 {
            return Err(ReleaseError::InvalidTrustRoot);
        }
        let values: Vec<String> =
            serde_json::from_str(value).map_err(|_| ReleaseError::InvalidTrustRoot)?;
        if values.is_empty() || values.len() > 8 {
            return Err(ReleaseError::InvalidTrustRoot);
        }
        let mut seen = BTreeSet::new();
        let mut keys = Vec::new();
        for value in values {
            if value.len() > 4096 || value.contains('\r') {
                return Err(ReleaseError::InvalidTrustRoot);
            }
            let lines: Vec<_> = value.lines().collect();
            let encoded = match lines.as_slice() {
                [encoded] => *encoded,
                [comment, encoded] if comment.starts_with("untrusted comment: ") => *encoded,
                _ => return Err(ReleaseError::InvalidTrustRoot),
            };
            let bytes = STANDARD
                .decode(encoded)
                .map_err(|_| ReleaseError::InvalidTrustRoot)?;
            if bytes.len() != 42
                || STANDARD.encode(&bytes) != encoded
                || !seen.insert(bytes[10..].to_vec())
            {
                return Err(ReleaseError::InvalidTrustRoot);
            }
            keys.push(PublicKey::from_base64(encoded).map_err(|_| ReleaseError::InvalidTrustRoot)?);
        }
        Ok(Self { keys })
    }
}

pub struct VerifiedRelease {
    metadata: ReleaseMetadata,
    checksums: BTreeMap<String, String>,
}

impl VerifiedRelease {
    pub fn metadata(&self) -> &ReleaseMetadata {
        &self.metadata
    }

    pub fn checksum(&self, path: &str) -> Option<&str> {
        self.checksums.get(path).map(String::as_str)
    }

    pub fn artifact(
        &self,
        name: &str,
        version: &str,
        arch: &str,
    ) -> Result<VerifiedArtifact, ReleaseError> {
        let path = canonical_path(name, version, arch)?;
        let metadata = self
            .metadata
            .artifacts
            .iter()
            .find(|entry| entry.name == name && entry.version == version && entry.arch == arch)
            .ok_or(ReleaseError::MissingArtifact)?
            .clone();
        let sha256 = self
            .checksums
            .get(&path)
            .ok_or(ReleaseError::MissingArtifact)?
            .clone();
        Ok(VerifiedArtifact {
            metadata,
            path,
            sha256,
        })
    }
}

#[derive(Clone)]
pub struct VerifiedArtifact {
    metadata: ReleaseArtifact,
    path: String,
    sha256: String,
}

impl VerifiedArtifact {
    pub fn metadata(&self) -> &ReleaseArtifact {
        &self.metadata
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn verify_archive(&self, bytes: &[u8]) -> Result<(), ReleaseError> {
        verify_bytes(bytes, self.metadata.archive_size, &self.sha256)
    }
    pub fn verify_binary(&self, bytes: &[u8]) -> Result<(), ReleaseError> {
        verify_bytes(
            bytes,
            self.metadata.binary_size,
            &self.metadata.binary_sha256,
        )
    }
}

fn verify_bytes(bytes: &[u8], size: u64, expected: &str) -> Result<(), ReleaseError> {
    if bytes.len() as u64 != size || digest(bytes) != expected {
        return Err(ReleaseError::BytesMismatch);
    }
    Ok(())
}

pub fn verify_release(
    proof: &ReleaseProof,
    keys: &TrustedKeys,
) -> Result<VerifiedRelease, ReleaseError> {
    if proof.checksums.is_empty()
        || proof.checksums.len() > MAX_CHECKSUMS_BYTES
        || proof.metadata_json.is_empty()
        || proof.metadata_json.len() > MAX_METADATA_BYTES
        || proof.signature.len() > MAX_SIGNATURE_BYTES
        || proof.signature.contains('\r')
    {
        return Err(ReleaseError::InvalidProof);
    }
    let lines: Vec<_> = proof.signature.lines().collect();
    if lines.len() != 4
        || !lines[0].starts_with("untrusted comment: ")
        || !lines[2].starts_with("trusted comment: ")
        || lines
            .iter()
            .any(|line| line.chars().any(|c| c.is_control() && c != '\t'))
    {
        return Err(ReleaseError::InvalidProof);
    }
    let signature = Signature::decode(&proof.signature).map_err(|_| ReleaseError::InvalidProof)?;
    if !keys.keys.iter().any(|key| {
        key.verify(proof.checksums.as_bytes(), &signature, false)
            .is_ok()
    }) {
        return Err(ReleaseError::UntrustedSignature);
    }
    let checksums = parse_checksums(&proof.checksums)?;
    if checksums.get("release.json") != Some(&digest(proof.metadata_json.as_bytes())) {
        return Err(ReleaseError::BytesMismatch);
    }
    let metadata: ReleaseMetadata =
        serde_json::from_str(&proof.metadata_json).map_err(|_| ReleaseError::InvalidMetadata)?;
    if metadata.schema != RELEASE_SCHEMA
        || metadata.source_repo != RELEASE_SOURCE_REPO
        || !safe_component(&metadata.tag)
        || metadata.protocol_min == 0
        || metadata.protocol_max < metadata.protocol_min
        || metadata.artifacts.is_empty()
        || metadata.artifacts.len() > 30
    {
        return Err(ReleaseError::InvalidMetadata);
    }
    let mut expected_paths = BTreeSet::from(["release.json".to_owned(), "install.sh".to_owned()]);
    let mut assets = BTreeSet::new();
    for artifact in &metadata.artifacts {
        let path = canonical_path(&artifact.name, &artifact.version, &artifact.arch)?;
        if !safe_component(&artifact.binary_name)
            || !valid_digest(&artifact.binary_sha256)
            || artifact.archive_size == 0
            || artifact.archive_size > MAX_ARTIFACT_BYTES
            || artifact.binary_size == 0
            || artifact.binary_size > MAX_BINARY_BYTES
            || artifact.asset_name != canonical_asset_name(artifact)?
            || artifact.auxiliary_files.len() > 7
            || artifact.auxiliary_files.iter().any(|(name, file)| {
                !safe_component(name)
                    || name == &artifact.binary_name
                    || matches!(
                        name.as_str(),
                        "release.json" | "SHA256SUMS" | "SHA256SUMS.minisig" | ".artifact.json"
                    )
                    || !valid_digest(&file.sha256)
                    || file.size == 0
                    || file.size > MAX_BINARY_BYTES
            })
            || (artifact.format == "raw" && !artifact.auxiliary_files.is_empty())
            || !expected_paths.insert(path.clone())
            || !assets.insert(artifact.asset_name.clone())
        {
            return Err(ReleaseError::InvalidMetadata);
        }
        if artifact.format == "raw"
            && (artifact.archive_size != artifact.binary_size
                || checksums.get(&path) != Some(&artifact.binary_sha256))
        {
            return Err(ReleaseError::InvalidMetadata);
        }
    }
    if checksums.keys().cloned().collect::<BTreeSet<_>>() != expected_paths {
        return Err(ReleaseError::InvalidChecksums);
    }
    Ok(VerifiedRelease {
        metadata,
        checksums,
    })
}

fn parse_checksums(value: &str) -> Result<BTreeMap<String, String>, ReleaseError> {
    if !value.ends_with('\n') || value.contains('\r') || value.lines().count() > 32 {
        return Err(ReleaseError::InvalidChecksums);
    }
    let mut checksums = BTreeMap::new();
    let mut previous = None;
    for line in value.lines() {
        let (hash, path) = line
            .split_once("  ")
            .ok_or(ReleaseError::InvalidChecksums)?;
        if !valid_digest(hash)
            || previous.is_some_and(|old| old >= path)
            || !(matches!(path, "release.json" | "install.sh")
                || {
                    let parts: Vec<_> = path.split('/').collect();
                    matches!(parts.as_slice(), [name, version, arch] if canonical_path(name, version, arch).is_ok_and(|valid| valid == path))
                })
        {
            return Err(ReleaseError::InvalidChecksums);
        }
        previous = Some(path);
        checksums.insert(path.to_owned(), hash.to_owned());
    }
    Ok(checksums)
}

pub fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !matches!(value, "." | "..")
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

pub fn canonical_path(name: &str, version: &str, arch: &str) -> Result<String, ReleaseError> {
    if !safe_component(name)
        || !safe_component(version)
        || !crate::platform::ARTIFACT_TARGETS.contains(&arch)
    {
        return Err(ReleaseError::IdentityMismatch);
    }
    Ok(format!("{name}/{version}/{arch}"))
}

pub fn canonical_asset_name(artifact: &ReleaseArtifact) -> Result<String, ReleaseError> {
    canonical_path(&artifact.name, &artifact.version, &artifact.arch)?;
    if !matches!(artifact.arch.as_str(), "amd64" | "arm64") {
        let extension = match artifact.format.as_str() {
            "raw" => "",
            "tar.gz" => ".tar.gz",
            _ => return Err(ReleaseError::InvalidMetadata),
        };
        return Ok(format!(
            "{}-{}-{}{extension}",
            artifact.name, artifact.version, artifact.arch
        ));
    }
    match artifact.format.as_str() {
        "raw" => Ok(format!(
            "{}-{}-linux-musl-{}",
            artifact.name, artifact.version, artifact.arch
        )),
        "tar.gz" => Ok(format!(
            "{}-{}-linux-{}.tar.gz",
            artifact.name, artifact.version, artifact.arch
        )),
        _ => Err(ReleaseError::InvalidMetadata),
    }
}

pub fn native_arch() -> Result<&'static str, ReleaseError> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("amd64"),
        "aarch64" => Ok("arm64"),
        _ => Err(ReleaseError::IdentityMismatch),
    }
}

pub fn native_target() -> Result<String, ReleaseError> {
    crate::platform::artifact_target(
        std::env::consts::OS,
        if cfg!(target_env = "musl") {
            Some("musl")
        } else {
            Some("gnu")
        },
        std::env::consts::ARCH,
    )
    .ok_or(ReleaseError::IdentityMismatch)
}

impl VerifiedRelease {
    pub fn native_artifact(
        &self,
        name: &str,
        version: &str,
    ) -> Result<VerifiedArtifact, ReleaseError> {
        let target = native_target()?;
        match self.artifact(name, version, &target) {
            Ok(artifact) => Ok(artifact),
            Err(ReleaseError::MissingArtifact) => self.artifact(name, version, native_arch()?),
            Err(error) => Err(error),
        }
    }
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

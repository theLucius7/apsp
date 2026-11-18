mod agents;
mod network;
mod selection;
mod storage;

pub use agents::selection_error;
pub use agents::{
    AgentInstallationPolicy, AgentVersion, AgentVersions, BootstrapVersionQuery, agent_versions,
    installable_agent_versions, list_agent_versions, select_agent_for_target,
};
pub use selection::agent_targets;

use crate::{
    AppState, control_center,
    error::{ApiError, ApiResult},
};
use anyhow::{Context, Result, ensure};
use apsp_protocol::{
    PROTOCOL_MAX, PROTOCOL_MIN,
    bundled::{
        EmbeddedAgentManifest, expected_embedded_component_digest,
        extract_embedded_agent_manifest_optional, valid_sha256,
    },
    release::{
        MAX_CHECKSUMS_BYTES, MAX_METADATA_BYTES, MAX_SIGNATURE_BYTES, ReleaseArtifact,
        ReleaseProof, TrustedKeys, VerifiedArtifact, VerifiedRelease, verify_release,
    },
};
use axum::{Json, extract::State, http::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    future::Future,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_AGENT_BINARY: usize = 128 * 1024 * 1024;
const MAX_INSTALLER: usize = 256 * 1024;
const FRAME_PREFIX_A: &[u8] = b"\0APSP_EMBEDDED_";
const FRAME_PREFIX_B: &[u8] = b"COMPONENTS_V1:";
const FRAME_SUFFIX_A: &[u8] = b"\n:END_APSP_EMBEDDED_";
const FRAME_SUFFIX_B: &[u8] = b"COMPONENTS_V1\0";

type Identity = (String, String, String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SignedAgent {
    pub name: &'static str,
    pub version: String,
    pub arch: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedAgentIdentity {
    pub version: String,
    pub tag: String,
    pub target: String,
    pub sha256: String,
    pub bytes: u64,
    pub source_repo: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedRunningComponent {
    pub agent_version: String,
    pub release_tag: String,
    pub target: String,
    pub source_repo: String,
    pub agent_sha256: String,
    pub agent_bytes: u64,
    pub component: String,
    pub component_version: String,
    pub component_sha256: String,
    pub static_info_received_at: i64,
}

struct StoredRelease {
    directory: PathBuf,
    proof: ReleaseProof,
    verified: VerifiedRelease,
    paths: BTreeSet<String>,
}

pub fn valid_tag(tag: &str) -> bool {
    tag.strip_prefix("agent-v").is_some_and(|version| {
        !version.is_empty()
            && version.len() <= 96
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
    })
}

pub(super) fn is_agent_entry(entry: &ReleaseArtifact) -> bool {
    let binary_name = if entry.arch.starts_with("windows-") {
        "apsp-agent.exe"
    } else {
        "apsp-agent"
    };
    entry.name == "agent"
        && entry.format == "raw"
        && entry.binary_name == binary_name
        && entry.auxiliary_files.is_empty()
        && entry.archive_size <= MAX_AGENT_BINARY as u64
}

fn requires_agent_only_manifest(tag: &str) -> bool {
    let Some(version) = tag.strip_prefix("agent-v") else {
        return true;
    };
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let mut parts = core.split('.');
    let (Some(major), Some(minor), Some(_patch), None) = (
        parts.next().and_then(|part| part.parse::<u64>().ok()),
        parts.next().and_then(|part| part.parse::<u64>().ok()),
        parts.next().and_then(|part| part.parse::<u64>().ok()),
        parts.next(),
    ) else {
        return true;
    };
    (major, minor) >= (0, 4)
}

pub(super) fn valid_release_entries(tag: &str, entries: &[ReleaseArtifact]) -> bool {
    let has_agent = entries.iter().any(|entry| entry.name == "agent");
    if !has_agent
        || !entries
            .iter()
            .filter(|entry| entry.name == "agent")
            .all(is_agent_entry)
    {
        return false;
    }
    if requires_agent_only_manifest(tag) {
        entries.iter().all(is_agent_entry)
    } else {
        true
    }
}

fn count_split_marker(haystack: &[u8], first: &[u8], second: &[u8]) -> usize {
    let marker_len = first.len() + second.len();
    if haystack.len() < marker_len {
        return 0;
    }
    (0..=haystack.len() - marker_len)
        .filter(|&offset| {
            &haystack[offset..offset + first.len()] == first
                && &haystack[offset + first.len()..offset + marker_len] == second
        })
        .count()
}

fn validate_embedded_manifest_target(
    manifest: &EmbeddedAgentManifest,
    expected_version: &str,
    expected_target: &str,
) -> Result<()> {
    ensure!(
        manifest.agent_version == expected_version,
        "embedded component manifest agent_version '{}' differs from signed version '{expected_version}'",
        manifest.agent_version
    );
    let expected_platform = match expected_target {
        "amd64" | "linux-musl-amd64" => ("linux", "x86_64", Some("musl")),
        "arm64" | "linux-musl-arm64" => ("linux", "aarch64", Some("musl")),
        "linux-gnu-amd64" => ("linux", "x86_64", Some("gnu")),
        "linux-gnu-arm64" => ("linux", "aarch64", Some("gnu")),
        "macos-arm64" => ("macos", "aarch64", None),
        "freebsd-amd64" => ("freebsd", "x86_64", None),
        "freebsd-arm64" => ("freebsd", "aarch64", None),
        "windows-amd64" => ("windows", "x86_64", None),
        "windows-arm64" => ("windows", "aarch64", None),
        _ => anyhow::bail!("unsupported signed Agent target '{expected_target}'"),
    };
    ensure!(
        manifest.target_os == expected_platform.0
            && manifest.target_arch == expected_platform.1
            && expected_platform
                .2
                .is_none_or(|env| manifest.target_env == env),
        "embedded component manifest platform ({}/{}/{}) differs from signed target '{expected_target}'",
        manifest.target_os,
        manifest.target_arch,
        manifest.target_env
    );
    Ok(())
}

/// Extracts and validates the unique structured `EmbeddedAgentManifest` frame from a verified
/// signed `apsp-agent` binary. Historical pre-0.4 Agent binaries with zero frames return `Ok(None)`;
/// binaries with multiple, truncated, or mismatched frames fail closed.
pub fn parse_embedded_manifest_frame(
    bytes: &[u8],
    expected_version: &str,
    expected_target: &str,
) -> Result<Option<EmbeddedAgentManifest>> {
    let prefixes = count_split_marker(bytes, FRAME_PREFIX_A, FRAME_PREFIX_B);
    let suffixes = count_split_marker(bytes, FRAME_SUFFIX_A, FRAME_SUFFIX_B);
    if prefixes == 0 && suffixes == 0 {
        return Ok(None);
    }
    ensure!(
        prefixes == 1 && suffixes == 1,
        "signed Agent binary must contain exactly one embedded component manifest frame"
    );
    let Some(manifest) =
        extract_embedded_agent_manifest_optional(bytes).map_err(anyhow::Error::msg)?
    else {
        return Ok(None);
    };
    validate_embedded_manifest_target(&manifest, expected_version, expected_target)?;
    Ok(Some(manifest))
}

fn keys(state: &AppState) -> ApiResult<&TrustedKeys> {
    state.release_keys.as_deref().ok_or_else(|| {
        ApiError::Conflict("面板构建时未配置发布公钥，请使用可信公钥重新构建".into())
    })
}

fn invalid(error: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %error, "signed release validation failed");
    ApiError::Conflict("发布签名、Agent 内容或路径校验失败，当前发布保持不变".into())
}

// Validate every existing ancestor before reading or creating managed storage.
fn checked(condition: bool, message: &str) -> ApiResult<()> {
    if condition {
        Ok(())
    } else {
        Err(invalid(message))
    }
}

async fn ordinary_directory(path: &Path, create: bool) -> Result<bool> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut prefix = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => continue,
            Component::ParentDir => {
                anyhow::bail!("release storage cannot contain parent components")
            }
            _ => prefix.push(component.as_os_str()),
        }
        let metadata = match tokio::fs::symlink_metadata(&prefix).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !create {
                    return Ok(false);
                }
                match tokio::fs::create_dir(&prefix).await {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
                    Err(error) => return Err(error.into()),
                }
                tokio::fs::symlink_metadata(&prefix).await?
            }
            Err(error) => return Err(error.into()),
        };
        ensure!(
            metadata.is_dir() && !metadata.is_symlink(),
            "release directory or ancestor must not be a symlink"
        );
    }
    Ok(true)
}

async fn ordinary_bytes(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    ensure!(
        ordinary_directory(path.parent().context("release file parent")?, false).await?,
        "release file parent is missing"
    );
    let metadata = tokio::fs::symlink_metadata(path).await?;
    ensure!(
        metadata.is_file() && !metadata.is_symlink() && metadata.len() <= maximum as u64,
        "release file must be a bounded ordinary file"
    );
    let bytes = tokio::fs::read(path).await?;
    ensure!(bytes.len() <= maximum, "release file grew beyond its limit");
    Ok(bytes)
}

pub async fn proof_at(directory: &Path) -> Result<ReleaseProof> {
    ensure!(
        ordinary_directory(directory, false).await?,
        "release directory is missing"
    );
    Ok(ReleaseProof {
        metadata_json: String::from_utf8(
            ordinary_bytes(&directory.join("release.json"), MAX_METADATA_BYTES).await?,
        )?,
        checksums: String::from_utf8(
            ordinary_bytes(&directory.join("SHA256SUMS"), MAX_CHECKSUMS_BYTES).await?,
        )?,
        signature: String::from_utf8(
            ordinary_bytes(&directory.join("SHA256SUMS.minisig"), MAX_SIGNATURE_BYTES).await?,
        )?,
    })
}

pub fn verify_payload(artifact: &VerifiedArtifact, bytes: &[u8]) -> Result<()> {
    let entry = artifact.metadata();
    ensure!(
        is_agent_entry(entry),
        "signed release entry must be a raw Agent binary"
    );
    artifact.verify_archive(bytes)?;
    artifact.verify_binary(bytes)?;
    let _ = parse_embedded_manifest_frame(bytes, &entry.version, &entry.arch)?;
    Ok(())
}

async fn released(state: &AppState) -> ApiResult<Vec<StoredRelease>> {
    let trusted = keys(state)?;
    let root = state.config.data_dir.join("artifacts/releases");
    if !ordinary_directory(&root, false).await.map_err(invalid)? {
        return Ok(Vec::new());
    }
    let mut directories = tokio::fs::read_dir(&root)
        .await
        .map_err(anyhow::Error::from)?;
    let mut releases = Vec::new();
    while let Some(directory) = directories
        .next_entry()
        .await
        .map_err(anyhow::Error::from)?
    {
        let tag = directory.file_name().to_string_lossy().into_owned();
        if !valid_tag(&tag) {
            continue;
        }
        if releases.len() >= 128 {
            return Err(ApiError::Conflict("发布数量超过扫描上限".into()));
        }
        let proof = proof_at(&directory.path()).await.map_err(invalid)?;
        let verified = verify_release(&proof, trusted).map_err(invalid)?;
        checked(
            verified.metadata().tag == tag,
            "release directory and signed tag differ",
        )?;
        let paths = storage::stored_paths(&directory.path(), &verified)
            .await
            .map_err(invalid)?;
        releases.push(StoredRelease {
            directory: directory.path(),
            proof,
            verified,
            paths,
        });
    }
    releases.sort_by(|a, b| a.directory.cmp(&b.directory));
    Ok(releases)
}

fn same_identity(old: &VerifiedArtifact, new: &VerifiedArtifact) -> bool {
    let a = old.metadata();
    let b = new.metadata();
    old.sha256() == new.sha256()
        && a.binary_sha256 == b.binary_sha256
        && a.archive_size == b.archive_size
        && a.binary_size == b.binary_size
        && a.binary_name == b.binary_name
        && a.format == b.format
        && a.auxiliary_files == b.auxiliary_files
}

fn inventory(releases: &[StoredRelease]) -> Result<BTreeMap<Identity, (usize, VerifiedArtifact)>> {
    let mut result = BTreeMap::new();
    for (index, release) in releases.iter().enumerate() {
        for entry in release
            .verified
            .metadata()
            .artifacts
            .iter()
            .filter(|entry| entry.name == "agent")
        {
            ensure!(
                is_agent_entry(entry),
                "signed Agent entry must use raw binary layout"
            );
            let key = (
                entry.name.clone(),
                entry.version.clone(),
                entry.arch.clone(),
            );
            let artifact = release
                .verified
                .artifact(&entry.name, &entry.version, &entry.arch)?;
            if let Some((_, old)) = result.get(&key) {
                ensure!(
                    same_identity(old, &artifact),
                    "an immutable Agent identity has different bytes"
                );
            } else {
                result.insert(key, (index, artifact));
            }
        }
    }
    Ok(result)
}

fn available_inventory(
    releases: &[StoredRelease],
) -> Result<BTreeMap<Identity, (usize, VerifiedArtifact)>> {
    // An absent payload still reserves its immutable identity in the signed proof.
    inventory(releases)?;
    let mut result = BTreeMap::new();
    for (index, release) in releases.iter().enumerate() {
        for entry in release
            .verified
            .metadata()
            .artifacts
            .iter()
            .filter(|entry| entry.name == "agent")
        {
            let artifact = release
                .verified
                .artifact(&entry.name, &entry.version, &entry.arch)?;
            if release.paths.contains(artifact.path()) {
                result
                    .entry((
                        entry.name.clone(),
                        entry.version.clone(),
                        entry.arch.clone(),
                    ))
                    .or_insert((index, artifact));
            }
        }
    }
    Ok(result)
}

type FileStamp = (u64, u64, u64, i64, i64, i64, i64);

const REVERIFY_AFTER: Duration = Duration::from_secs(60 * 60);
const SETTLED_AFTER: Duration = Duration::from_secs(2);
const STATIC_INFO_FRESH_SECS: i64 = 600;

struct CachedPayload {
    stamp: FileStamp,
    verified_at: Instant,
    manifest: Option<Arc<EmbeddedAgentManifest>>,
}

fn payload_cache() -> &'static Mutex<HashMap<(PathBuf, String), CachedPayload>> {
    static CACHE: OnceLock<Mutex<HashMap<(PathBuf, String), CachedPayload>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

#[cfg(unix)]
fn file_stamp(metadata: &std::fs::Metadata) -> Option<FileStamp> {
    use std::os::unix::fs::MetadataExt;
    Some((
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
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

fn stamp_settled(stamp: &FileStamp, started: SystemTime) -> bool {
    let (Ok(seconds), Ok(nanoseconds)) = (u64::try_from(stamp.5), u32::try_from(stamp.6)) else {
        return false;
    };
    UNIX_EPOCH
        .checked_add(Duration::new(seconds, nanoseconds))
        .and_then(|changed| started.duration_since(changed).ok())
        .is_some_and(|age| age >= SETTLED_AFTER)
}

async fn ensure_stored_payload(
    release: &StoredRelease,
    artifact: &VerifiedArtifact,
) -> Result<Option<Arc<EmbeddedAgentManifest>>> {
    let path = release.directory.join(artifact.path());
    ensure!(
        ordinary_directory(path.parent().context("release file parent")?, false).await?,
        "release file parent is missing"
    );
    let started = SystemTime::now();
    let metadata = tokio::fs::symlink_metadata(&path).await?;
    ensure!(
        metadata.is_file()
            && !metadata.is_symlink()
            && metadata.len() == artifact.metadata().archive_size
            && metadata.len() <= MAX_AGENT_BINARY as u64,
        "release payload must match its signed size"
    );
    let stamp = file_stamp(&metadata);
    let cache_key = (path.clone(), artifact.sha256().to_owned());
    if let Some(stamp) = stamp
        && let Ok(cache) = payload_cache().lock()
        && let Some(cached) = cache.get(&cache_key)
        && cached.stamp == stamp
        && cached.verified_at.elapsed() < REVERIFY_AFTER
    {
        return Ok(cached.manifest.clone());
    }
    let bytes = ordinary_bytes(&path, MAX_AGENT_BINARY).await?;
    let entry = artifact.metadata();
    ensure!(
        is_agent_entry(entry),
        "signed release entry must be a raw Agent binary"
    );
    artifact.verify_archive(&bytes)?;
    artifact.verify_binary(&bytes)?;
    let manifest =
        parse_embedded_manifest_frame(&bytes, &entry.version, &entry.arch)?.map(Arc::new);
    let after = tokio::fs::symlink_metadata(&path).await?;
    if let Some(after_stamp) = file_stamp(&after)
        && Some(after_stamp) == stamp
        && stamp_settled(&after_stamp, started)
        && let Ok(mut cache) = payload_cache().lock()
    {
        if cache.len() >= 256 {
            cache.clear();
        }
        cache.insert(
            cache_key,
            CachedPayload {
                stamp: after_stamp,
                verified_at: Instant::now(),
                manifest: manifest.clone(),
            },
        );
    }
    Ok(manifest)
}

pub async fn signed_agents(state: &AppState) -> ApiResult<Vec<SignedAgent>> {
    let releases = released(state).await?;
    let mut values = Vec::new();
    for (_, (index, artifact)) in available_inventory(&releases).map_err(invalid)? {
        ensure_stored_payload(&releases[index], &artifact)
            .await
            .map_err(invalid)?;
        let entry = artifact.metadata();
        values.push(SignedAgent {
            name: "agent",
            version: entry.version.clone(),
            arch: entry.arch.clone(),
            sha256: artifact.sha256().to_owned(),
            bytes: entry.archive_size,
        });
    }
    Ok(values)
}

/// Resolves the signed Agent release identity matching the device's reported `static_info`
/// (`agent_version`, `os`, `arch`, `libc`, and optional running `agent_sha256`) against verified
/// release proofs in `state`.
pub async fn verified_agent_for_static_info(
    state: &AppState,
    static_info: &Value,
) -> ApiResult<Option<VerifiedAgentIdentity>> {
    let Some(version) = static_info["agent_version"]
        .as_str()
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let candidates = agent_targets(static_info);
    if candidates.is_empty() {
        return Ok(None);
    }
    let reported_sha = reported_agent_sha256(static_info);
    let releases = released(state).await?;
    let all = inventory(&releases).map_err(invalid)?;
    for target in candidates {
        let Some((index, artifact)) =
            all.get(&("agent".into(), version.to_owned(), target.clone()))
        else {
            continue;
        };
        let release = &releases[*index];
        let metadata = release.verified.metadata();
        if !is_agent_entry(artifact.metadata()) || metadata.tag != format!("agent-v{version}") {
            continue;
        }
        if reported_sha.is_some_and(|sha| artifact.sha256() != sha) {
            continue;
        }
        if release.paths.contains(artifact.path()) {
            ensure_stored_payload(release, artifact)
                .await
                .map_err(invalid)?;
        }
        return Ok(Some(VerifiedAgentIdentity {
            version: version.to_owned(),
            tag: metadata.tag.clone(),
            target,
            sha256: artifact.sha256().to_owned(),
            bytes: artifact.metadata().archive_size,
            source_repo: metadata.source_repo.clone(),
        }));
    }
    Ok(None)
}

pub fn reported_agent_sha256(static_info: &Value) -> Option<&str> {
    let primary = static_info["component_digests"]["agent"].as_str();
    let alias = static_info["agent_sha256"].as_str();
    match (primary, alias) {
        (Some(a), Some(b)) if a != b => None,
        (Some(a), _) => valid_sha256(a).then_some(a),
        (None, Some(b)) => valid_sha256(b).then_some(b),
        (None, None) => None,
    }
}

/// Verifies that an online authenticated device is running an exact signed `apsp-agent`
/// binary from the panel's verified release catalog AND that its reported embedded
/// `component` (`"sing-box"`, `"ipquality"`, `"nodequality"`, `"tcpquality"`,
/// `"network-workbench"`) name, version, and digest match the unique structured
/// `EmbeddedAgentManifest` inside that exact verified Agent binary.
pub async fn verify_running_component(
    state: &AppState,
    static_info: &Value,
    static_info_received_at: Option<i64>,
    last_seen: Option<i64>,
    now: i64,
    component: &str,
    expected_component_version: &str,
) -> ApiResult<VerifiedRunningComponent> {
    if !last_seen.is_some_and(|seen| seen <= now + 60 && now.saturating_sub(seen) <= 60) {
        return Err(ApiError::Conflict(
            "Agent 当前离线，无法核实运行中的签名整包身份".into(),
        ));
    }
    let Some(received_at) = static_info_received_at.filter(|at| {
        *at > 0 && *at <= now + 60 && now.saturating_sub(*at) <= STATIC_INFO_FRESH_SECS
    }) else {
        return Err(ApiError::Conflict(
            "尚未收到当前会话的新鲜设备组件身份上报，请等待设备同步".into(),
        ));
    };
    let Some(version) = static_info["agent_version"]
        .as_str()
        .filter(|value| !value.is_empty())
    else {
        return Err(ApiError::Conflict(
            "设备尚未上报当前 Agent 版本，无法核实签名整包身份".into(),
        ));
    };
    let Some(agent_sha256) = reported_agent_sha256(static_info) else {
        return Err(ApiError::Conflict(
            "设备尚未上报运行中 Agent 二进制摘要，请升级至当前一体化 Agent 版本".into(),
        ));
    };
    if component.is_empty()
        || expected_component_version.is_empty()
        || static_info["components"][component].as_str() != Some(expected_component_version)
    {
        return Err(ApiError::Conflict(format!(
            "设备上报的内置 {component} 版本不匹配固定可信版本 {expected_component_version}"
        )));
    }
    let Some(component_sha256) = static_info["component_digests"][component]
        .as_str()
        .filter(|digest| valid_sha256(digest))
    else {
        return Err(ApiError::Conflict(format!(
            "设备尚未上报内置 {component} 组件摘要，无法冻结可信身份"
        )));
    };
    let candidates = agent_targets(static_info);
    if candidates.is_empty() {
        return Err(ApiError::Conflict(
            "设备平台信息不完整，无法匹配签名 Agent".into(),
        ));
    }
    let releases = released(state).await?;
    let all = inventory(&releases).map_err(invalid)?;
    let mut any_imported_for_version = false;
    let mut matched = None;
    for target in candidates {
        if let Some((index, artifact)) =
            all.get(&("agent".into(), version.to_owned(), target.clone()))
        {
            let release = &releases[*index];
            if is_agent_entry(artifact.metadata())
                && release.verified.metadata().tag == format!("agent-v{version}")
            {
                any_imported_for_version = true;
                if artifact.sha256() == agent_sha256 {
                    matched = Some((target, release, artifact));
                    break;
                }
            }
        }
    }
    if !any_imported_for_version {
        return Err(ApiError::Conflict(
            "当前设备运行的 Agent 版本尚未导入面板签名 Release 目录".into(),
        ));
    }
    let Some((target, release, artifact)) = matched else {
        return Err(ApiError::Conflict(
            "设备实际运行的 Agent 二进制摘要与签名 Release 目录不一致，拒绝未授权或本地重编包"
                .into(),
        ));
    };
    if !release.paths.contains(artifact.path()) {
        return Err(ApiError::Conflict(
            "面板尚未缓存此目标平台的已验签 Agent 发布，无法核实内置组件摘要归属".into(),
        ));
    }
    let Some(manifest) = ensure_stored_payload(release, artifact)
        .await
        .map_err(invalid)?
    else {
        return Err(ApiError::Conflict(
            "当前运行的签名 Agent 未内嵌结构化组件清单，请升级至当前一体化 Agent 版本".into(),
        ));
    };
    let Some(entry) = manifest.components.get(component) else {
        return Err(ApiError::Conflict(format!(
            "已验签的 Agent 整包结构化清单未包含内置组件 {component}"
        )));
    };
    if entry.version != expected_component_version {
        return Err(ApiError::Conflict(format!(
            "已验签的 Agent 整包内置 {component} 版本为 {}，不匹配期望版本 {expected_component_version}",
            entry.version
        )));
    }
    let Some(expected_digest) =
        expected_embedded_component_digest(component, entry, artifact.sha256())
    else {
        return Err(invalid(
            "embedded component manifest entry format is invalid",
        ));
    };
    if expected_digest != component_sha256 {
        return Err(ApiError::Conflict(format!(
            "设备上报的内置 {component} 摘要与已验签 Agent 整包结构化清单不一致"
        )));
    }
    let metadata = release.verified.metadata();
    Ok(VerifiedRunningComponent {
        agent_version: version.to_owned(),
        release_tag: metadata.tag.clone(),
        target,
        source_repo: metadata.source_repo.clone(),
        agent_sha256: agent_sha256.to_owned(),
        agent_bytes: artifact.metadata().archive_size,
        component: component.to_owned(),
        component_version: expected_component_version.to_owned(),
        component_sha256: component_sha256.to_owned(),
        static_info_received_at: received_at,
    })
}

pub async fn select_agent(state: &AppState, version: Option<&str>) -> ApiResult<(String, String)> {
    select_agent_for_target(state, version, None).await
}

/// Selects an Agent for the pinned independent Linux bootstrap.
/// Release installers remain signed evidence; the bootstrap runs its own trusted executor.
pub async fn select_installable_agent(
    state: &AppState,
    version: Option<&str>,
) -> ApiResult<(String, String)> {
    select_agent_for_target(state, version, None).await
}

/// Returns only protocol-compatible, signed updates for the requested ABI.
pub async fn newer_agent(
    state: &AppState,
    targets: &[String],
    current: (u64, u64, u64),
) -> ApiResult<Option<apsp_protocol::AgentRelease>> {
    let releases = released(state).await?;
    // Agent bytes come from GitHub, so the full signed proof authorizes uncached targets.
    let inventory = inventory(&releases).map_err(invalid)?;
    let mut candidates = Vec::new();
    for ((name, version, target), (index, artifact)) in inventory {
        let metadata = releases[index].verified.metadata();
        if name != "agent"
            || metadata.protocol_min > PROTOCOL_MAX
            || metadata.protocol_max < PROTOCOL_MIN
        {
            continue;
        }
        let Some(priority) = targets.iter().position(|value| value == &target) else {
            continue;
        };
        let Some(key) = apsp_protocol::release_version(&version).filter(|key| *key > current)
        else {
            continue;
        };
        candidates.push((
            key,
            std::cmp::Reverse(priority),
            version,
            target,
            index,
            artifact,
        ));
    }
    candidates.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    let Some((_, _, version, _, index, artifact)) = candidates.pop() else {
        return Ok(None);
    };
    let release = &releases[index];
    Ok(Some(apsp_protocol::AgentRelease {
        version: version.clone(),
        download_mirror: String::new(),
        artifact: apsp_protocol::Artifact {
            url: format!(
                "https://github.com/{}/releases/download/{}/{}",
                release.verified.metadata().source_repo,
                release.verified.metadata().tag,
                artifact.metadata().asset_name
            ),
            sha256: artifact.sha256().into(),
            proof: Some(release.proof.clone()),
        },
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub tag: String,
    pub targets: Option<Vec<String>>,
}

async fn synced_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    Ok(())
}

async fn sync_directory(path: &Path) -> Result<()> {
    ensure!(
        ordinary_directory(path, false).await?,
        "release directory is missing"
    );
    tokio::fs::File::open(path).await?.sync_all().await?;
    Ok(())
}

/// Validates and atomically stores a release. Callers provide bytes, never arbitrary URLs.
/// Only the HTTP handler binds the asset loader to the fixed GitHub repository.
pub async fn import_bundle<F, Fut>(
    state: &AppState,
    tag: &str,
    proof: ReleaseProof,
    asset_bytes: F,
) -> ApiResult<usize>
where
    F: FnMut(String, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    let targets = vec![apsp_protocol::release::native_target().map_err(invalid)?];
    import_bundle_for_targets(state, tag, proof, &targets, asset_bytes).await
}

pub async fn import_bundle_for_targets<F, Fut>(
    state: &AppState,
    tag: &str,
    proof: ReleaseProof,
    targets: &[String],
    asset_bytes: F,
) -> ApiResult<usize>
where
    F: FnMut(String, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    if !valid_tag(tag) {
        return Err(ApiError::BadRequest(
            "请输入 agent-v 开头的规范发布标签".into(),
        ));
    }
    let _permit = state
        .release_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::Busy)?;
    selection::validate_targets(targets)?;
    store_bundle(state, tag, proof, targets, asset_bytes).await
}

async fn store_bundle<F, Fut>(
    state: &AppState,
    tag: &str,
    proof: ReleaseProof,
    targets: &[String],
    asset_bytes: F,
) -> ApiResult<usize>
where
    F: FnMut(String, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    let verified = verify_release(&proof, keys(state)?).map_err(invalid)?;
    checked(verified.metadata().tag == tag, "signed tag differs")?;
    checked(
        valid_release_entries(tag, &verified.metadata().artifacts),
        "signed release entries are invalid for this release line",
    )?;
    let paths = selection::selected_paths(&verified, targets)?;
    let releases = released(state).await?;
    let existing = inventory(&releases).map_err(invalid)?;
    for entry in verified
        .metadata()
        .artifacts
        .iter()
        .filter(|entry| entry.name == "agent")
    {
        if let Some((_, old)) = existing.get(&(
            entry.name.clone(),
            entry.version.clone(),
            entry.arch.clone(),
        )) {
            let artifact = verified
                .artifact(&entry.name, &entry.version, &entry.arch)
                .map_err(invalid)?;
            checked(
                same_identity(old, &artifact),
                "an immutable Agent identity already has different bytes",
            )?;
        }
    }
    let old = releases
        .iter()
        .find(|release| release.verified.metadata().tag == tag);
    if let Some(old) = old {
        checked(
            old.proof.checksums == proof.checksums,
            "immutable release already differs",
        )?;
    }
    storage::store(state, tag, proof, verified, paths, old, asset_bytes).await
}

pub async fn target_options(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    Ok(Json(json!({
        "default_targets": selection::default_targets(&state).await?,
        "supported_targets": apsp_protocol::platform::ARTIFACT_TARGETS,
    })))
}

pub async fn import(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ImportRequest>,
) -> ApiResult<Json<Value>> {
    control_center::require_owner(&state, &headers).await?;
    control_center::require_recent_proof(&state, &headers).await?;
    if !valid_tag(&request.tag) {
        return Err(ApiError::BadRequest(
            "请输入 agent-v 开头的规范发布标签".into(),
        ));
    }
    let targets = match request.targets {
        Some(targets) => targets,
        None => selection::default_targets(&state).await?,
    };
    selection::validate_targets(&targets)?;
    let _permit = state
        .release_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::Busy)?;
    let proof = network::release_proof(&request.tag, keys(&state)?)
        .await
        .map_err(invalid)?;
    let tag = request.tag.clone();
    let count = store_bundle(
        &state,
        &request.tag,
        proof,
        &targets,
        move |asset, maximum| {
            let tag = tag.clone();
            async move { network::asset(&tag, &asset, maximum).await }
        },
    )
    .await?;
    let version = request.tag.strip_prefix("agent-v").unwrap_or(&request.tag);
    Ok(Json(json!({
        "tag": request.tag,
        "version": version,
        "targets": targets,
        "artifacts": count,
        "signature_verified": true,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apsp_protocol::bundled::{EmbeddedComponentRecord, encode_embedded_agent_manifest_frame};

    fn sample_manifest() -> EmbeddedAgentManifest {
        EmbeddedAgentManifest {
            schema: 1,
            agent_version: "0.4.0".into(),
            target_os: "linux".into(),
            target_arch: "x86_64".into(),
            target_env: "musl".into(),
            components: BTreeMap::from([
                (
                    "sing-box".into(),
                    EmbeddedComponentRecord {
                        format: "tar.gz".into(),
                        sha256: Some("1".repeat(64)),
                        subcommand: None,
                        version: "1.14.4-apsp.2".into(),
                    },
                ),
                (
                    "ipquality".into(),
                    EmbeddedComponentRecord {
                        format: "tar.gz".into(),
                        sha256: Some("2".repeat(64)),
                        subcommand: None,
                        version: "87397e2c3196ec796f5477c83343c2354df601ea-node-r1".into(),
                    },
                ),
                (
                    "tcpquality".into(),
                    EmbeddedComponentRecord {
                        format: "agent_subcommand".into(),
                        sha256: None,
                        subcommand: Some("tcp-probe".into()),
                        version: format!("0.3.0-{}-r1", "a".repeat(40)),
                    },
                ),
            ]),
        }
    }

    #[test]
    fn embedded_manifest_frame_requires_exact_single_frame_and_ignores_unrelated_hex64() {
        let manifest = sample_manifest();
        let mut binary = Vec::new();
        binary.extend_from_slice(b"\x7fELF-agent-prefix ");
        // An arbitrary 64-char hex constant in .rodata must never be treated as a component digest.
        binary.extend_from_slice("f".repeat(64).as_bytes());
        binary.push(b' ');
        let frame = encode_embedded_agent_manifest_frame(&manifest).unwrap();
        binary.extend_from_slice(&frame);
        binary.extend_from_slice(b" trailing-rodata");

        let parsed = parse_embedded_manifest_frame(&binary, "0.4.0", "linux-musl-amd64")
            .unwrap()
            .expect("structured embedded manifest");
        assert_eq!(parsed, manifest);
        assert_eq!(
            expected_embedded_component_digest(
                "sing-box",
                &parsed.components["sing-box"],
                &"9".repeat(64)
            ),
            Some("1".repeat(64).as_str())
        );
        assert_eq!(
            expected_embedded_component_digest(
                "tcpquality",
                &parsed.components["tcpquality"],
                &"9".repeat(64)
            ),
            Some("9".repeat(64).as_str())
        );

        // Zero frames on a legacy 0.3 binary parses to None, never inventing components.
        let legacy = format!("legacy-agent-{}", "e".repeat(64));
        assert!(
            parse_embedded_manifest_frame(legacy.as_bytes(), "0.3.9", "amd64")
                .unwrap()
                .is_none()
        );

        // Duplicate frames, wrong target ABI, or tampered payload bytes fail closed.
        let mut duplicate = binary.clone();
        duplicate.extend_from_slice(&frame);
        assert!(parse_embedded_manifest_frame(&duplicate, "0.4.0", "linux-musl-amd64").is_err());
        assert!(parse_embedded_manifest_frame(&binary, "0.4.0", "linux-gnu-amd64").is_err());
        assert!(parse_embedded_manifest_frame(&binary, "0.4.1", "linux-musl-amd64").is_err());

        let mut corrupted = frame;
        let payload_offset = 103 + 2;
        corrupted[payload_offset] ^= 1;
        assert!(parse_embedded_manifest_frame(&corrupted, "0.4.0", "linux-musl-amd64").is_err());
    }

    #[test]
    fn reported_agent_digest_rejects_conflicting_primary_and_alias_fields() {
        let digest_a = "a".repeat(64);
        let digest_b = "b".repeat(64);
        assert_eq!(
            reported_agent_sha256(&json!({"component_digests": {"agent": digest_a}})),
            Some(digest_a.as_str())
        );
        assert_eq!(
            reported_agent_sha256(&json!({
                "component_digests": {"agent": digest_a},
                "agent_sha256": digest_a
            })),
            Some(digest_a.as_str())
        );
        assert!(
            reported_agent_sha256(&json!({
                "component_digests": {"agent": digest_a},
                "agent_sha256": digest_b
            }))
            .is_none()
        );
    }
}

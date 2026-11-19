#![allow(dead_code)]

use crate::release_support as signing;

use anyhow::{Context, Result, bail, ensure};
#[allow(unused_imports)]
pub use apsp_protocol::bundled::{
    EmbeddedAgentManifest, EmbeddedComponentRecord, encode_embedded_agent_manifest_frame,
    expected_embedded_component_digest,
};
use apsp_protocol::release::{
    ReleaseArtifact, ReleaseMetadata, ReleaseProof, canonical_asset_name,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedTestAgent {
    pub release_dir: PathBuf,
    pub agent_version: String,
    pub release_tag: String,
    pub target: String,
    pub agent_sha256: String,
    pub agent_bytes: u64,
    pub raw_binary: Vec<u8>,
    pub components: BTreeMap<String, String>,
    pub component_digests: BTreeMap<String, String>,
}

impl SignedTestAgent {
    pub fn merge_static_info(&self, existing: &Value) -> Value {
        let mut info = if existing.is_object() {
            existing.clone()
        } else {
            json!({})
        };
        let (os, arch, _, libc) =
            target_platform(&self.target).expect("validated signed Agent target");
        let map = info.as_object_mut().expect("JSON object");
        map.insert("agent_version".into(), json!(self.agent_version));
        map.insert("os".into(), json!(os));
        map.insert("arch".into(), json!(arch));
        match libc {
            Some(libc) => {
                map.insert("libc".into(), json!(libc));
            }
            None => {
                map.remove("libc");
            }
        }
        map.insert("components".into(), json!(self.components));
        map.insert("component_digests".into(), json!(self.component_digests));
        info
    }
}

pub fn component_record(name: &str, version: &str) -> (String, EmbeddedComponentRecord) {
    if name == "tcpquality" {
        (
            name.to_owned(),
            EmbeddedComponentRecord {
                format: "agent_subcommand".into(),
                inventory_sha256: None,
                sha256: None,
                subcommand: Some("tcp-probe".into()),
                version: version.to_owned(),
            },
        )
    } else {
        let sha256 = signing::hash(format!("TEST_ONLY embedded {name}:{version}").as_bytes());
        (
            name.to_owned(),
            EmbeddedComponentRecord {
                format: "tar.gz".into(),
                inventory_sha256: None,
                sha256: Some(sha256),
                subcommand: None,
                version: version.to_owned(),
            },
        )
    }
}

pub fn pinned_components(names: &[&str]) -> Result<BTreeMap<String, EmbeddedComponentRecord>> {
    let mut components = BTreeMap::new();
    for &name in names {
        let version = match name {
            "sing-box" => apsp_traffic::RUNTIME_VERSION,
            "ipquality" => apsp_observability::diagnostic_tools::ipquality::TOOL_VERSION,
            "nodequality" => apsp_observability::diagnostic_tools::nodequality::TOOL_VERSION,
            "network-workbench" => "1.0.0",
            "tcpquality" => apsp_observability::diagnostic_tools::tcpquality::TOOL_VERSION,
            other => bail!("unknown pinned embedded component '{other}'"),
        };
        let (key, record) = component_record(name, version);
        components.insert(key, record);
    }
    Ok(components)
}

pub fn build_signed_agent_entry(
    agent_version: &str,
    target: &str,
    components: BTreeMap<String, EmbeddedComponentRecord>,
) -> Result<(ReleaseArtifact, SignedTestAgent)> {
    let (target_os, target_arch, target_env, _) = target_platform(target)?;
    let manifest = EmbeddedAgentManifest {
        schema: 1,
        agent_version: agent_version.to_owned(),
        target_os: target_os.into(),
        target_arch: target_arch.into(),
        target_env: target_env.into(),
        components: components.clone(),
    };
    let frame = encode_embedded_agent_manifest_frame(&manifest).map_err(anyhow::Error::msg)?;
    let header = format!("\x7fELF\x02\x01TEST_ONLY signed apsp-agent {agent_version} {target}:");
    let mut raw_binary = header.into_bytes();
    raw_binary.extend_from_slice(&frame);
    raw_binary.extend_from_slice(b":end-test-only-agent");
    let binary_name = if target.starts_with("windows-") {
        "apsp-agent.exe"
    } else {
        "apsp-agent"
    };
    let mut entry = signing::entry(
        "agent",
        agent_version,
        binary_name,
        "raw",
        &raw_binary,
        &raw_binary,
    );
    entry.arch = target.to_owned();
    entry.asset_name = canonical_asset_name(&entry)?;
    let agent_sha256 = entry.binary_sha256.clone();
    let agent_bytes = entry.archive_size;
    let mut versions = BTreeMap::new();
    let mut digests = BTreeMap::from([("agent".to_owned(), agent_sha256.clone())]);
    for (name, record) in &components {
        let digest = expected_embedded_component_digest(name, record, &agent_sha256)
            .with_context(|| format!("invalid embedded component record for '{name}'"))?;
        versions.insert(name.clone(), record.version.clone());
        digests.insert(name.clone(), digest.to_owned());
    }
    Ok((
        entry,
        SignedTestAgent {
            release_dir: PathBuf::new(),
            agent_version: agent_version.to_owned(),
            release_tag: format!("agent-v{agent_version}"),
            target: target.to_owned(),
            agent_sha256,
            agent_bytes,
            raw_binary,
            components: versions,
            component_digests: digests,
        },
    ))
}

pub fn write_signed_agent(
    root: &Path,
    agent_version: &str,
    target: &str,
    components: BTreeMap<String, EmbeddedComponentRecord>,
) -> Result<SignedTestAgent> {
    write_signed_agents(root, agent_version, &[target], &components)?
        .pop()
        .context("signed test Agent")
}

pub fn write_signed_agents(
    root: &Path,
    agent_version: &str,
    targets: &[&str],
    components: &BTreeMap<String, EmbeddedComponentRecord>,
) -> Result<Vec<SignedTestAgent>> {
    ensure!(
        !targets.is_empty(),
        "signed Agent fixture requires at least one target"
    );
    let release_tag = format!("agent-v{agent_version}");
    let mut artifacts = Vec::with_capacity(targets.len());
    let mut built = Vec::with_capacity(targets.len());
    for &target in targets {
        let (entry, agent) = build_signed_agent_entry(agent_version, target, components.clone())?;
        artifacts.push((entry, agent.raw_binary.clone()));
        built.push(agent);
    }
    let release_dir = write_entries_tagged(root, &release_tag, artifacts)?;
    for agent in &mut built {
        agent.release_dir = release_dir.clone();
    }
    Ok(built)
}

pub fn write(
    root: &Path,
    name: &str,
    version: &str,
    binary_name: &str,
    archive: &[u8],
    binary: &[u8],
    format: &str,
) -> Result<PathBuf> {
    let mut artifacts: Vec<(ReleaseArtifact, Vec<u8>)> = Vec::new();
    for arch in ["amd64", "arm64"] {
        let mut entry = signing::entry(name, version, binary_name, format, archive, binary);
        entry.arch = arch.into();
        entry.asset_name = canonical_asset_name(&entry)?;
        artifacts.push((entry, archive.to_vec()));
    }
    let release = write_entries(root, artifacts)?;
    Ok(release.join(name).join(version))
}

pub fn archive(binary_name: &str, binary: &[u8]) -> Result<Vec<u8>> {
    let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_size(binary.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    archive.append_data(&mut header, binary_name, binary)?;
    Ok(archive.into_inner()?.finish()?)
}

pub fn write_entries(root: &Path, artifacts: Vec<(ReleaseArtifact, Vec<u8>)>) -> Result<PathBuf> {
    write_entries_tagged(root, "agent-v0.3.0", artifacts)
}

pub fn write_entries_tagged(
    root: &Path,
    tag: &str,
    artifacts: Vec<(ReleaseArtifact, Vec<u8>)>,
) -> Result<PathBuf> {
    let proof = signed_with_tag_and_installer(tag, artifacts.clone(), MODERN_INSTALLER)?;
    let release = root.join("artifacts/releases").join(tag);
    std::fs::create_dir_all(&release)?;
    signing::install_proof(&release, &proof);
    std::fs::write(release.join("install.sh"), MODERN_INSTALLER)?;
    for (entry, bytes) in artifacts {
        let directory = release.join(&entry.name).join(&entry.version);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(directory.join(&entry.arch), bytes)?;
    }
    Ok(release)
}

const MODERN_INSTALLER: &[u8] =
    b"#!/bin/sh\n# APSP_BOOTSTRAP_AGENT_SOURCE=preloaded-github-v1\nexit 0\n";

fn target_platform(
    target: &str,
) -> Result<(
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
)> {
    Ok(match target {
        "amd64" => ("linux", "x86_64", "musl", None),
        "linux-musl-amd64" => ("linux", "x86_64", "musl", Some("musl")),
        "arm64" => ("linux", "aarch64", "musl", None),
        "linux-musl-arm64" => ("linux", "aarch64", "musl", Some("musl")),
        "linux-gnu-amd64" => ("linux", "x86_64", "gnu", Some("gnu")),
        "linux-gnu-arm64" => ("linux", "aarch64", "gnu", Some("gnu")),
        "macos-arm64" => ("macos", "aarch64", "", None),
        "freebsd-amd64" => ("freebsd", "x86_64", "", None),
        "freebsd-arm64" => ("freebsd", "aarch64", "", None),
        "windows-amd64" => ("windows", "x86_64", "", None),
        "windows-arm64" => ("windows", "aarch64", "", None),
        _ => bail!("unsupported signed Agent target '{target}'"),
    })
}

fn signed_with_tag_and_installer(
    tag: &str,
    artifacts: Vec<(ReleaseArtifact, Vec<u8>)>,
    installer: &[u8],
) -> Result<ReleaseProof> {
    let mut proof = signing::signed_release(artifacts);
    if tag != "agent-v0.3.0" {
        let mut metadata: ReleaseMetadata = serde_json::from_str(&proof.metadata_json)?;
        metadata.tag = tag.to_owned();
        proof.metadata_json = format!("{}\n", serde_json::to_string(&metadata)?);
    }
    update_installer_proof(&mut proof, installer);
    Ok(proof)
}

fn update_installer_proof(proof: &mut ReleaseProof, installer: &[u8]) {
    let metadata_hash = signing::hash(proof.metadata_json.as_bytes());
    let installer_hash = signing::hash(installer);
    proof.checksums = proof
        .checksums
        .lines()
        .map(|line| {
            if line.ends_with("  release.json") {
                format!("{metadata_hash}  release.json\n")
            } else if line.ends_with("  install.sh") {
                format!("{installer_hash}  install.sh\n")
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    proof.signature = signing::sign(proof.checksums.as_bytes());
}

pub fn replace_signed_installer(directory: &Path, installer: &[u8]) -> Result<()> {
    let mut proof = ReleaseProof {
        metadata_json: std::fs::read_to_string(directory.join("release.json"))?,
        checksums: std::fs::read_to_string(directory.join("SHA256SUMS"))?,
        signature: std::fs::read_to_string(directory.join("SHA256SUMS.minisig"))?,
    };
    update_installer_proof(&mut proof, installer);
    signing::install_proof(directory, &proof);
    std::fs::write(directory.join("install.sh"), installer)?;
    Ok(())
}

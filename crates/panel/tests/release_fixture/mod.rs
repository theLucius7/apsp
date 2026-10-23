#![allow(dead_code)]

use crate::release_support as signing;

use anyhow::Result;
use apsp_protocol::release::{ReleaseArtifact, canonical_asset_name};
use std::path::{Path, PathBuf};

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
    let proof = signed_with_installer(artifacts.clone(), MODERN_INSTALLER);
    let release = root.join("artifacts/releases/agent-v0.3.0");
    std::fs::create_dir_all(&release)?;
    signing::install_proof(&release, &proof);
    std::fs::write(release.join("install.sh"), MODERN_INSTALLER)?;
    for (entry, bytes) in artifacts {
        let directory = release.join(&entry.name).join(&entry.version);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(directory.join(&entry.arch), bytes)?;
    }
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
    let proof = signed_with_installer(artifacts.clone(), MODERN_INSTALLER);
    let release = root.join("artifacts/releases/agent-v0.3.0");
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

fn signed_with_installer(
    artifacts: Vec<(ReleaseArtifact, Vec<u8>)>,
    installer: &[u8],
) -> apsp_protocol::release::ReleaseProof {
    let mut proof = signing::signed_release(artifacts);
    update_installer_proof(&mut proof, installer);
    proof
}

fn update_installer_proof(proof: &mut apsp_protocol::release::ReleaseProof, installer: &[u8]) {
    proof.checksums = proof
        .checksums
        .lines()
        .map(|line| {
            if line.ends_with("  install.sh") {
                format!("{}  install.sh\n", signing::hash(installer))
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    proof.signature = signing::sign(proof.checksums.as_bytes());
}

pub fn replace_signed_installer(directory: &Path, installer: &[u8]) -> Result<()> {
    let mut proof = apsp_protocol::release::ReleaseProof {
        metadata_json: std::fs::read_to_string(directory.join("release.json"))?,
        checksums: std::fs::read_to_string(directory.join("SHA256SUMS"))?,
        signature: std::fs::read_to_string(directory.join("SHA256SUMS.minisig"))?,
    };
    update_installer_proof(&mut proof, installer);
    signing::install_proof(directory, &proof);
    std::fs::write(directory.join("install.sh"), installer)?;
    Ok(())
}

#![forbid(unsafe_code)]

use apsp_protocol::{
    bundled::{
        BUNDLED_MARKER, BundledFileRecord, BundledManifest, EmbeddedAgentManifest,
        EmbeddedComponentRecord, MAX_BUNDLED_FILES, MAX_BUNDLED_UNPACKED_BYTES,
        encode_embedded_agent_manifest_frame, normalize_bundled_relative_path, valid_sha256,
        valid_version_segment, validate_binary_abi, validate_bundled_component_contents,
    },
    release::MAX_AGENT_RAW_BYTES,
};
use flate2::read::MultiGzDecoder;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};

const DEFAULT_SINGBOX_VERSION: &str = "1.14.4-apsp.2";
const DEFAULT_NODEQUALITY_VERSION: &str = "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r22";
const DEFAULT_IPQUALITY_VERSION: &str = "87397e2c3196ec796f5477c83343c2354df601ea-node-r1";
const DEFAULT_NETWORK_WORKBENCH_VERSION: &str = "1.0.0";
const DEFAULT_TCPQUALITY_ENGINE_VERSION: &str = "0.3.0";

struct EmbeddedArchive {
    version: String,
    sha256: String,
    inventory_sha256: String,
    staged_filename: String,
}

struct BuildTarget<'a> {
    os: &'a str,
    arch: &'a str,
    env: &'a str,
    release_required: bool,
}

fn non_empty_env(name: &str) -> Option<String> {
    println!("cargo:rerun-if-env-changed={name}");
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn is_truthy_flag(value: Option<String>) -> bool {
    value.is_some_and(|text| {
        matches!(
            text.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn is_lowercase_hex(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn resolve_git_head_commit() -> Option<String> {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR")?);
    let repo_root = manifest_dir.join("../..");
    let git_head = repo_root.join(".git/HEAD");
    if git_head.exists() {
        println!("cargo:rerun-if-changed={}", git_head.display());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(&repo_root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    is_lowercase_hex(&commit, 40).then_some(commit)
}

fn binary_name_for_component(component: &str, target_os: &str) -> Result<&'static str, String> {
    match component {
        "sing-box" => Ok(if target_os == "windows" {
            "sing-box.exe"
        } else {
            "sing-box"
        }),
        "nodequality" => Ok("nodequality"),
        "ipquality" => Ok("ipquality"),
        "network-workbench" => Ok("apsp-network-workbench"),
        other => Err(format!("unsupported embedded component '{other}'")),
    }
}

fn ensure_elf_has_no_pt_interp(bytes: &[u8], label: &str) -> Result<(), String> {
    if bytes.len() < 64 || &bytes[0..6] != b"\x7fELF\x02\x01" {
        return Err(format!("{label} is not a 64-bit little-endian ELF binary"));
    }
    let phoff = usize::try_from(u64::from_le_bytes(
        bytes[32..40]
            .try_into()
            .map_err(|_| format!("{label} ELF phoff slice failed"))?,
    ))
    .map_err(|_| format!("{label} ELF phoff overflow"))?;
    let phentsize = usize::from(u16::from_le_bytes([bytes[54], bytes[55]]));
    let phnum = usize::from(u16::from_le_bytes([bytes[56], bytes[57]]));
    let table_end = phoff
        .checked_add(
            phentsize
                .checked_mul(phnum)
                .ok_or_else(|| format!("{label} ELF program header overflow"))?,
        )
        .ok_or_else(|| format!("{label} ELF program header end overflow"))?;
    if phentsize < 56 || table_end > bytes.len() {
        return Err(format!("{label} has invalid ELF64 program headers"));
    }
    for index in 0..phnum {
        let offset = phoff + index * phentsize;
        let p_type = u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]);
        if p_type == 3 {
            return Err(format!(
                "{label} contains PT_INTERP dynamic interpreter; expected static musl ELF"
            ));
        }
    }
    Ok(())
}

fn inspect_component_tar_gz(
    component: &str,
    version: &str,
    target_os: &str,
    archive_sha256: &str,
    bytes: &[u8],
) -> Result<(BundledManifest, BTreeMap<String, Vec<u8>>), String> {
    if bytes.len() < 18 || bytes[0] != 0x1f || bytes[1] != 0x8b {
        return Err(format!("{component} archive is not a valid gzip stream"));
    }
    let primary_binary = binary_name_for_component(component, target_os)?;
    let decoder = MultiGzDecoder::new(bytes).take(MAX_BUNDLED_UNPACKED_BYTES + 1);
    let mut archive = tar::Archive::new(decoder);
    let mut files = BTreeMap::new();
    let mut records = BTreeMap::new();
    let mut unpacked_total: u64 = 0;

    let entries = archive
        .entries()
        .map_err(|error| format!("failed to read {component} tar entries: {error}"))?;
    for entry_result in entries {
        let mut entry =
            entry_result.map_err(|error| format!("invalid {component} tar entry: {error}"))?;
        let entry_type = entry.header().entry_type();
        let raw_path = entry
            .path()
            .map_err(|error| format!("invalid {component} entry path: {error}"))?
            .into_owned();
        if entry_type.is_dir() {
            let trimmed = raw_path
                .to_str()
                .ok_or_else(|| format!("{component} directory path is not UTF-8"))?
                .trim_start_matches("./")
                .trim_end_matches('/');
            if !trimmed.is_empty() && trimmed != "." {
                let _ = normalize_bundled_relative_path(Path::new(trimmed))?;
            }
            continue;
        }
        if !entry_type.is_file() {
            return Err(format!(
                "{component} archive entry '{}' must be an ordinary file (got {entry_type:?})",
                raw_path.display()
            ));
        }
        let raw_text = raw_path
            .to_str()
            .ok_or_else(|| format!("{component} entry path is not UTF-8"))?;
        let cleaned = raw_text.strip_prefix("./").unwrap_or(raw_text);
        let normalized = normalize_bundled_relative_path(Path::new(cleaned))?;
        if normalized == BUNDLED_MARKER {
            return Err(format!(
                "{component} archive cannot contain reserved '{BUNDLED_MARKER}'"
            ));
        }
        if records.contains_key(&normalized) {
            return Err(format!(
                "{component} archive contains duplicate member '{normalized}'"
            ));
        }
        if records.len() >= MAX_BUNDLED_FILES {
            return Err(format!(
                "{component} archive exceeds member file ceiling ({MAX_BUNDLED_FILES})"
            ));
        }
        let declared_size = entry
            .header()
            .size()
            .map_err(|error| format!("invalid {component} size for '{normalized}': {error}"))?;
        unpacked_total = unpacked_total
            .checked_add(declared_size)
            .filter(|sum| *sum <= MAX_BUNDLED_UNPACKED_BYTES)
            .ok_or_else(|| {
                format!(
                    "{component} archive exceeds unpacked size ceiling ({MAX_BUNDLED_UNPACKED_BYTES} bytes)"
                )
            })?;
        let mode = entry.header().mode().unwrap_or(0o644);
        let executable = normalized == primary_binary || (mode & 0o111) != 0;
        if executable && declared_size == 0 {
            return Err(format!(
                "{component} executable member '{normalized}' must not be empty"
            ));
        }
        let mut content = Vec::with_capacity(
            usize::try_from(declared_size)
                .map_err(|_| format!("{component} member '{normalized}' size overflow"))?,
        );
        entry.read_to_end(&mut content).map_err(|error| {
            format!("failed reading {component} member '{normalized}': {error}")
        })?;
        if content.len() as u64 != declared_size {
            return Err(format!(
                "{component} archive member '{normalized}' length mismatch"
            ));
        }
        let file_sha256 = format!("{:x}", Sha256::digest(&content));
        records.insert(
            normalized.clone(),
            BundledFileRecord {
                sha256: file_sha256,
                size: declared_size,
                executable,
            },
        );
        files.insert(normalized, content);
    }

    let mut decoder = archive.into_inner();
    let mut tail = [0_u8; 8192];
    loop {
        let count = decoder
            .read(&mut tail)
            .map_err(|error| format!("failed reading {component} archive trailer: {error}"))?;
        if count == 0 {
            break;
        }
        if tail[..count].iter().any(|byte| *byte != 0) {
            return Err(format!("{component} archive contains trailing data"));
        }
    }
    if decoder.limit() == 0 {
        return Err(format!(
            "{component} archive exceeds unpacked size ceiling ({MAX_BUNDLED_UNPACKED_BYTES} bytes)"
        ));
    }

    let binary_record = records.get(primary_binary).ok_or_else(|| {
        format!("{component} archive is missing primary binary '{primary_binary}'")
    })?;
    let manifest = BundledManifest {
        schema: 1,
        component_name: component.to_owned(),
        binary_name: primary_binary.to_owned(),
        version: version.to_owned(),
        bundle_sha256: archive_sha256.to_owned(),
        binary_sha256: binary_record.sha256.clone(),
        files: records,
    };
    Ok((manifest, files))
}

fn expected_linux_arch_name(target_arch: &str) -> Result<&'static str, String> {
    match target_arch {
        "x86_64" => Ok("amd64"),
        "aarch64" => Ok("arm64"),
        other => Err(format!("unsupported Linux target_arch '{other}'")),
    }
}

fn validate_component_archive(
    component: &str,
    version: &str,
    target: &BuildTarget<'_>,
    archive_sha256: &str,
    bytes: &[u8],
) -> Result<BundledManifest, String> {
    let (manifest, files) =
        inspect_component_tar_gz(component, version, target.os, archive_sha256, bytes)?;
    validate_bundled_component_contents(&manifest, target.os, target.arch, target.env, |name| {
        files.get(name).map(Vec::as_slice)
    })?;

    match component {
        "sing-box" => {
            if target.os == "linux" && target.env == "musl" {
                let binary = files
                    .get("sing-box")
                    .ok_or_else(|| "sing-box archive missing 'sing-box'".to_owned())?;
                ensure_elf_has_no_pt_interp(binary, "sing-box")?;
            }
        }
        "nodequality" => {
            let runner = files
                .get("nodequality")
                .ok_or_else(|| "nodequality archive missing 'nodequality'".to_owned())?;
            if !(runner.starts_with(b"#!") || runner.starts_with(b"\x7fELF")) {
                return Err(
                    "nodequality entrypoint must be an executable script or ELF binary".into(),
                );
            }
            if runner.starts_with(b"\x7fELF") {
                validate_binary_abi(runner, target.os, target.arch, target.env, false)?;
            }
            if target.release_required
                && version == DEFAULT_NODEQUALITY_VERSION
                && !files.contains_key("runtime-manifest.json")
            {
                return Err(
                    "official Linux release requires self-contained nodequality -r22 archive with runtime-manifest.json"
                        .into(),
                );
            }
            if let Some(rt_manifest_bytes) = files.get("runtime-manifest.json") {
                if target.os == "linux" {
                    let rt: Value = serde_json::from_slice(rt_manifest_bytes).map_err(|error| {
                        format!("invalid nodequality runtime-manifest.json: {error}")
                    })?;
                    let expected_arch = expected_linux_arch_name(target.arch)?;
                    if let Some(arch) = rt.get("arch").and_then(Value::as_str)
                        && arch != expected_arch
                    {
                        return Err(format!(
                            "nodequality runtime-manifest.json arch '{arch}' does not match '{expected_arch}'"
                        ));
                    }
                }
            } else if files.contains_key("rootfs.tar.gz") {
                let rootfs = &files["rootfs.tar.gz"];
                if rootfs.len() < 18 || rootfs[0] != 0x1f || rootfs[1] != 0x8b {
                    return Err("nodequality rootfs.tar.gz is not a valid gzip stream".into());
                }
                let rootfs_manifest: Value = serde_json::from_slice(&files["rootfs-manifest.json"])
                    .map_err(|error| {
                        format!("invalid nodequality rootfs-manifest.json: {error}")
                    })?;
                if !rootfs_manifest.is_object() {
                    return Err("nodequality rootfs-manifest.json must be a JSON object".into());
                }
            }
        }
        "ipquality" => {
            let rootfs = &files["rootfs.tar.gz"];
            if rootfs.len() < 18 || rootfs[0] != 0x1f || rootfs[1] != 0x8b {
                return Err("ipquality rootfs.tar.gz is not a valid gzip stream".into());
            }
            let info: Value = serde_json::from_slice(&files["build-info.json"])
                .map_err(|error| format!("invalid ipquality build-info.json: {error}"))?;
            let expected_arch = expected_linux_arch_name(target.arch)?;
            if info.get("schema").and_then(Value::as_u64) != Some(1)
                || info.get("plugin").and_then(Value::as_str) != Some("ipquality")
                || info.get("version").and_then(Value::as_str) != Some(version)
                || info.get("arch").and_then(Value::as_str) != Some(expected_arch)
                || info.get("profile").and_then(Value::as_str) != Some("ipquality-node-v1")
            {
                return Err("ipquality build-info.json identity or architecture mismatch".into());
            }
        }
        "network-workbench" => {
            let wb: Value =
                serde_json::from_slice(&files["tools-manifest.json"]).map_err(|error| {
                    format!("invalid network-workbench tools-manifest.json: {error}")
                })?;
            if wb.get("engine_version").and_then(Value::as_str) != Some(version) {
                return Err("network-workbench engine_version mismatch".into());
            }
            let interp_obj = wb["interpreter"]
                .as_object()
                .ok_or_else(|| "network-workbench interpreter must be an object".to_owned())?;
            let interp_path = normalize_bundled_relative_path(Path::new(
                interp_obj
                    .get("bundled_path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        "network-workbench missing interpreter.bundled_path".to_owned()
                    })?,
            ))?;
            let interp_sha = interp_obj
                .get("sha256")
                .and_then(Value::as_str)
                .filter(|digest| valid_sha256(digest))
                .ok_or_else(|| "network-workbench interpreter missing valid sha256".to_owned())?;
            let interp_bytes = &files[&interp_path];
            if manifest.files[&interp_path].sha256 != interp_sha {
                return Err("network-workbench interpreter sha256 mismatch".into());
            }
            if target.os == "linux" && interp_bytes.starts_with(b"\x7fELF") {
                validate_binary_abi(interp_bytes, target.os, target.arch, target.env, false)?;
            }
            let tools = wb["tools"]
                .as_object()
                .ok_or_else(|| "network-workbench tools must be an object".to_owned())?;
            for (tool_name, entry) in tools {
                if entry.get("path").is_some() {
                    return Err(format!(
                        "embedded network-workbench tool '{tool_name}' cannot retain host-only 'path'"
                    ));
                }
                if !entry
                    .get("source_url")
                    .and_then(Value::as_str)
                    .is_some_and(|url| url.starts_with("https://"))
                    || entry
                        .get("license")
                        .and_then(Value::as_str)
                        .is_none_or(str::is_empty)
                {
                    return Err(format!(
                        "network-workbench tool '{tool_name}' lacks license or HTTPS source evidence"
                    ));
                }
                let expected_sha = entry
                    .get("sha256")
                    .and_then(Value::as_str)
                    .filter(|digest| valid_sha256(digest))
                    .ok_or_else(|| {
                        format!("network-workbench tool '{tool_name}' invalid sha256")
                    })?;
                if !entry
                    .get("version_output_sha256")
                    .and_then(Value::as_str)
                    .is_some_and(valid_sha256)
                {
                    return Err(format!(
                        "network-workbench tool '{tool_name}' missing version_output_sha256"
                    ));
                }
                let tool_path = normalize_bundled_relative_path(Path::new(
                    entry["bundled_path"]
                        .as_str()
                        .ok_or_else(|| format!("tool '{tool_name}' missing bundled_path"))?,
                ))?;
                if manifest.files[&tool_path].sha256 != expected_sha {
                    return Err(format!(
                        "network-workbench tool '{tool_name}' sha256 mismatch"
                    ));
                }
                let tool_bytes = &files[&tool_path];
                if target.os == "linux" && tool_bytes.starts_with(b"\x7fELF") {
                    validate_binary_abi(tool_bytes, target.os, target.arch, target.env, false)?;
                }
            }
        }
        other => return Err(format!("unsupported embedded component '{other}'")),
    }
    Ok(manifest)
}

fn stage_archive(
    component: &str,
    archive_path: Option<String>,
    version: String,
    required: bool,
    target: &BuildTarget<'_>,
    out_dir: &Path,
    cumulative_bytes: &mut u64,
) -> Option<EmbeddedArchive> {
    if !valid_version_segment(&version) {
        panic!("invalid embedded version '{version}' for component '{component}'");
    }
    let Some(raw_path) = archive_path else {
        if required {
            panic!(
                "release build requires embedded archive for '{component}' on target '{}-{}', but its archive environment variable was not set",
                target.os, target.arch
            );
        }
        return None;
    };

    let source_path = PathBuf::from(&raw_path);
    println!("cargo:rerun-if-changed={}", source_path.display());
    let metadata = fs::symlink_metadata(&source_path).unwrap_or_else(|error| {
        panic!(
            "failed to inspect embedded {component} archive '{}': {error}",
            source_path.display()
        )
    });
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        panic!(
            "embedded {component} archive '{}' must be an ordinary non-symlink file",
            source_path.display()
        );
    }
    if metadata.len() == 0 {
        panic!(
            "embedded {component} archive '{}' must not be empty",
            source_path.display()
        );
    }
    *cumulative_bytes = cumulative_bytes.saturating_add(metadata.len());
    if *cumulative_bytes >= MAX_AGENT_RAW_BYTES {
        panic!(
            "embedded archives ({} bytes) exceed raw Agent update limit ({MAX_AGENT_RAW_BYTES} bytes)",
            *cumulative_bytes
        );
    }

    let bytes = fs::read(&source_path).unwrap_or_else(|error| {
        panic!(
            "failed to read embedded {component} archive '{}': {error}",
            source_path.display()
        )
    });
    if bytes.len() as u64 != metadata.len() {
        panic!(
            "embedded {component} archive '{}' changed size while reading",
            source_path.display()
        );
    }
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let inventory_sha256 = validate_component_archive(component, &version, target, &sha256, &bytes)
        .and_then(|manifest| manifest.inventory_sha256())
        .unwrap_or_else(|error| panic!("invalid embedded {component} archive: {error}"));

    let staged_filename = format!("embedded-{component}.tar.gz");
    let staged_path = out_dir.join(&staged_filename);
    fs::write(&staged_path, &bytes).unwrap_or_else(|error| {
        panic!(
            "failed to stage embedded {component} archive to '{}': {error}",
            staged_path.display()
        )
    });

    Some(EmbeddedArchive {
        version,
        sha256,
        inventory_sha256,
        staged_filename,
    })
}

fn render_constant(name: &str, entry: Option<&EmbeddedArchive>) -> String {
    match entry {
        Some(archive) => format!(
            "pub const {name}: Option<(&str, &str, &str, &[u8])> = Some(({:?}, {:?}, {:?}, include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{}\"))));\n",
            archive.version, archive.sha256, archive.inventory_sha256, archive.staged_filename
        ),
        None => format!("pub const {name}: Option<(&str, &str, &str, &[u8])> = None;\n"),
    }
}

fn main() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set by Cargo"));
    let agent_version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION must be set");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let release_required = is_truthy_flag(non_empty_env("APSP_RELEASE_EMBED_REQUIRED"));
    let linux_release_required = release_required && target_os == "linux";

    let singbox_archive = non_empty_env("APSP_EMBEDDED_SINGBOX_ARCHIVE");
    let singbox_version = non_empty_env("APSP_EMBEDDED_SINGBOX_VERSION")
        .unwrap_or_else(|| DEFAULT_SINGBOX_VERSION.to_owned());

    let nodequality_archive = non_empty_env("APSP_EMBEDDED_NODEQUALITY_ARCHIVE");
    let nodequality_version = non_empty_env("APSP_EMBEDDED_NODEQUALITY_VERSION")
        .unwrap_or_else(|| DEFAULT_NODEQUALITY_VERSION.to_owned());

    let ipquality_archive = non_empty_env("APSP_EMBEDDED_IPQUALITY_ARCHIVE");
    let ipquality_version = non_empty_env("APSP_EMBEDDED_IPQUALITY_VERSION")
        .unwrap_or_else(|| DEFAULT_IPQUALITY_VERSION.to_owned());

    let workbench_archive = non_empty_env("APSP_EMBEDDED_NETWORK_WORKBENCH_ARCHIVE");
    let workbench_version = non_empty_env("APSP_EMBEDDED_NETWORK_WORKBENCH_VERSION")
        .unwrap_or_else(|| DEFAULT_NETWORK_WORKBENCH_VERSION.to_owned());

    let tcp_source_commit =
        non_empty_env("APSP_NATIVE_TCP_SOURCE_COMMIT").or_else(resolve_git_head_commit);
    if release_required
        && tcp_source_commit
            .as_deref()
            .is_none_or(|c| !is_lowercase_hex(c, 40))
    {
        panic!("release build requires a 40-char lowercase hex APSP_NATIVE_TCP_SOURCE_COMMIT");
    }
    let tcpquality_version = non_empty_env("APSP_EMBEDDED_TCPQUALITY_VERSION").unwrap_or_else(
        || match tcp_source_commit.as_deref() {
            Some(commit) if is_lowercase_hex(commit, 40) => {
                format!("{DEFAULT_TCPQUALITY_ENGINE_VERSION}-{commit}-r1")
            }
            _ => DEFAULT_TCPQUALITY_ENGINE_VERSION.to_owned(),
        },
    );
    if !valid_version_segment(&tcpquality_version) {
        panic!("invalid embedded tcpquality version '{tcpquality_version}'");
    }

    let build_target = BuildTarget {
        os: &target_os,
        arch: &target_arch,
        env: &target_env,
        release_required,
    };
    let mut cumulative_bytes = 0u64;
    let singbox = stage_archive(
        "sing-box",
        singbox_archive,
        singbox_version,
        release_required,
        &build_target,
        &out_dir,
        &mut cumulative_bytes,
    );
    let nodequality = stage_archive(
        "nodequality",
        nodequality_archive,
        nodequality_version,
        linux_release_required,
        &build_target,
        &out_dir,
        &mut cumulative_bytes,
    );
    let ipquality = stage_archive(
        "ipquality",
        ipquality_archive,
        ipquality_version,
        linux_release_required,
        &build_target,
        &out_dir,
        &mut cumulative_bytes,
    );
    let workbench = stage_archive(
        "network-workbench",
        workbench_archive,
        workbench_version,
        linux_release_required,
        &build_target,
        &out_dir,
        &mut cumulative_bytes,
    );

    let mut manifest_components = BTreeMap::new();
    for (name, staged) in [
        ("sing-box", singbox.as_ref()),
        ("nodequality", nodequality.as_ref()),
        ("ipquality", ipquality.as_ref()),
        ("network-workbench", workbench.as_ref()),
    ] {
        if let Some(archive) = staged {
            manifest_components.insert(
                name.to_owned(),
                EmbeddedComponentRecord {
                    format: "tar.gz".to_owned(),
                    inventory_sha256: Some(archive.inventory_sha256.clone()),
                    sha256: Some(archive.sha256.clone()),
                    subcommand: None,
                    version: archive.version.clone(),
                },
            );
        }
    }
    if target_os == "linux" {
        manifest_components.insert(
            "tcpquality".to_owned(),
            EmbeddedComponentRecord {
                format: "agent_subcommand".to_owned(),
                inventory_sha256: None,
                sha256: None,
                subcommand: Some("tcp-probe".to_owned()),
                version: tcpquality_version.clone(),
            },
        );
    }
    let embedded_manifest = EmbeddedAgentManifest {
        schema: 1,
        agent_version,
        target_os,
        target_arch,
        target_env,
        components: manifest_components,
    };
    let frame_bytes = encode_embedded_agent_manifest_frame(&embedded_manifest)
        .unwrap_or_else(|error| panic!("invalid embedded Agent manifest: {error}"));
    let frame_path = out_dir.join("embedded-components-frame.bin");
    fs::write(&frame_path, &frame_bytes).unwrap_or_else(|error| {
        panic!(
            "failed to write embedded component manifest frame '{}': {error}",
            frame_path.display()
        )
    });

    let mut generated = String::new();
    generated.push_str(&render_constant("EMBEDDED_SINGBOX", singbox.as_ref()));
    generated.push_str(&render_constant(
        "EMBEDDED_NODEQUALITY",
        nodequality.as_ref(),
    ));
    generated.push_str(&render_constant("EMBEDDED_IPQUALITY", ipquality.as_ref()));
    generated.push_str(&render_constant(
        "EMBEDDED_NETWORK_WORKBENCH",
        workbench.as_ref(),
    ));
    generated.push_str(&format!(
        "pub const EMBEDDED_TCPQUALITY_VERSION: &str = {:?};\n",
        tcpquality_version
    ));
    generated.push_str(
        "pub static EMBEDDED_COMPONENTS_FRAME: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/embedded-components-frame.bin\"));\n",
    );

    let generated_path = out_dir.join("embedded_bundle.rs");
    fs::write(&generated_path, generated).unwrap_or_else(|error| {
        panic!(
            "failed to write generated bundle metadata '{}': {error}",
            generated_path.display()
        )
    });
}

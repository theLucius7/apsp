use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

pub const BUNDLED_MARKER: &str = ".bundled.json";
pub const MAX_BUNDLED_MARKER_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_BUNDLED_FILES: usize = 4096;
pub const MAX_BUNDLED_UNPACKED_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_BUNDLED_PATH_DEPTH: usize = 12;
pub const MAX_EMBEDDED_MANIFEST_JSON_BYTES: usize = 65_536;

static FRAME_PREFIX_A: [u8; 16] = *b"\0APSP_EMBEDDED_!";
static FRAME_PREFIX_B: [u8; 15] = *b"!COMPONENTS_V1:";
static FRAME_TRAILER_A: [u8; 21] = *b"\n:END_APSP_EMBEDDED_!";
static FRAME_TRAILER_B: [u8; 15] = *b"!COMPONENTS_V1\0";

fn frame_prefix_parts() -> (&'static [u8], &'static [u8]) {
    (
        std::hint::black_box(&FRAME_PREFIX_A[..15]),
        std::hint::black_box(&FRAME_PREFIX_B[1..]),
    )
}

fn frame_trailer_parts() -> (&'static [u8], &'static [u8]) {
    (
        std::hint::black_box(&FRAME_TRAILER_A[..20]),
        std::hint::black_box(&FRAME_TRAILER_B[1..]),
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddedAgentManifest {
    pub schema: u32,
    pub agent_version: String,
    pub target_os: String,
    pub target_arch: String,
    pub target_env: String,
    pub components: BTreeMap<String, EmbeddedComponentRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddedComponentRecord {
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subcommand: Option<String>,
    pub version: String,
}

impl EmbeddedAgentManifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err("embedded component manifest schema must equal 1".into());
        }
        if !valid_version_segment(&self.agent_version)
            || !valid_segment(&self.target_os)
            || !valid_segment(&self.target_arch)
            || (!self.target_env.is_empty() && !valid_segment(&self.target_env))
        {
            return Err("invalid embedded component manifest header fields".into());
        }
        if self.components.len() > 16 {
            return Err("embedded component manifest exceeds component limit".into());
        }
        for (name, entry) in &self.components {
            if !valid_segment(name) || !valid_version_segment(&entry.version) {
                return Err(format!(
                    "invalid embedded component name or version for '{name}'"
                ));
            }
            match entry.format.as_str() {
                "tar.gz" => {
                    if name == "tcpquality"
                        || entry.subcommand.is_some()
                        || entry
                            .inventory_sha256
                            .as_deref()
                            .is_some_and(|value| !valid_sha256(value))
                        || !entry.sha256.as_deref().is_some_and(valid_sha256)
                    {
                        return Err(format!(
                            "embedded tar.gz component '{name}' must bind a valid SHA256 without subcommand"
                        ));
                    }
                }
                "agent_subcommand" => {
                    if name != "tcpquality"
                        || entry.subcommand.as_deref() != Some("tcp-probe")
                        || entry.sha256.is_some()
                        || entry.inventory_sha256.is_some()
                    {
                        return Err(format!(
                            "embedded agent_subcommand component '{name}' must bind tcp-probe without self-digest"
                        ));
                    }
                }
                other => {
                    return Err(format!("unsupported embedded component format '{other}'"));
                }
            }
        }
        Ok(())
    }
}

/// Derives the authoritative SHA256 for an embedded component without self-referential cycles:
/// `tar.gz` components carry their archive SHA256; `agent_subcommand` (`tcp-probe`) derives
/// its executable SHA256 directly from the verified outer Agent binary SHA256.
pub fn expected_embedded_component_digest<'a>(
    component: &str,
    entry: &'a EmbeddedComponentRecord,
    verified_agent_sha256: &'a str,
) -> Option<&'a str> {
    if !valid_sha256(verified_agent_sha256) {
        return None;
    }
    match entry.format.as_str() {
        "tar.gz" if component != "tcpquality" && entry.subcommand.is_none() => entry
            .sha256
            .as_deref()
            .filter(|digest| valid_sha256(digest)),
        "agent_subcommand"
            if component == "tcpquality"
                && entry.subcommand.as_deref() == Some("tcp-probe")
                && entry.sha256.is_none() =>
        {
            Some(verified_agent_sha256)
        }
        _ => None,
    }
}

pub fn encode_embedded_agent_manifest_frame(
    manifest: &EmbeddedAgentManifest,
) -> Result<Vec<u8>, String> {
    manifest.validate()?;
    let json_bytes =
        serde_json::to_vec(manifest).map_err(|error| format!("serialize manifest: {error}"))?;
    if json_bytes.is_empty() || json_bytes.len() > MAX_EMBEDDED_MANIFEST_JSON_BYTES {
        return Err("embedded Agent manifest JSON size outside 1..=65536".into());
    }
    let json_sha256 = format!("{:x}", Sha256::digest(&json_bytes));
    let (prefix_a, prefix_b) = frame_prefix_parts();
    let (trailer_a, trailer_b) = frame_trailer_parts();
    let mut frame = Vec::with_capacity(103 + json_bytes.len() + 34);
    frame.extend_from_slice(prefix_a);
    frame.extend_from_slice(prefix_b);
    frame.extend_from_slice(format!("{:08x}:{json_sha256}\n", json_bytes.len()).as_bytes());
    frame.extend_from_slice(&json_bytes);
    frame.extend_from_slice(trailer_a);
    frame.extend_from_slice(trailer_b);
    Ok(frame)
}

fn matches_split_at(bytes: &[u8], anchor: usize, first: &[u8], second: &[u8]) -> bool {
    bytes.get(anchor..anchor.saturating_add(second.len())) == Some(second)
        && anchor
            .checked_sub(first.len())
            .and_then(|start| bytes.get(start..anchor))
            == Some(first)
}

pub fn extract_embedded_agent_manifest_optional(
    bytes: &[u8],
) -> Result<Option<EmbeddedAgentManifest>, String> {
    let (prefix_a, prefix_b) = frame_prefix_parts();
    let (trailer_a, trailer_b) = frame_trailer_parts();
    debug_assert_eq!(prefix_b.first(), Some(&b'C'));
    debug_assert_eq!(trailer_b.first(), Some(&b'C'));
    let prefix_len = prefix_a.len() + prefix_b.len();
    let trailer_len = trailer_a.len() + trailer_b.len();
    let mut prefix_start = None;
    let mut trailer_start = None;
    for pos in memchr::memchr_iter(b'C', bytes) {
        if matches_split_at(bytes, pos, prefix_a, prefix_b)
            && prefix_start.replace(pos - prefix_a.len()).is_some()
        {
            return Err(
                "signed Agent binary must contain at most one embedded component manifest frame"
                    .into(),
            );
        }
        if matches_split_at(bytes, pos, trailer_a, trailer_b)
            && trailer_start.replace(pos - trailer_a.len()).is_some()
        {
            return Err(
                "signed Agent binary must contain at most one embedded component manifest frame"
                    .into(),
            );
        }
    }
    let start = match (prefix_start, trailer_start) {
        (None, None) => return Ok(None),
        (None, Some(_)) => {
            return Err(
                "signed Agent binary contains an orphan embedded component manifest frame trailer"
                    .into(),
            );
        }
        (Some(start), _) => start,
    };
    let header_end = start
        .checked_add(103)
        .ok_or_else(|| "embedded component manifest header overflow".to_owned())?;
    let header = bytes
        .get(start..header_end)
        .ok_or_else(|| "truncated embedded component manifest frame header".to_owned())?;
    if header[prefix_len + 8] != b':' || header[102] != b'\n' {
        return Err("malformed embedded component manifest frame header".into());
    }
    let len_hex = std::str::from_utf8(&header[prefix_len..prefix_len + 8])
        .map_err(|_| "embedded component manifest length is not ASCII".to_owned())?;
    if !len_hex
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("embedded component manifest length must be lowercase hex".into());
    }
    let json_len = usize::from_str_radix(len_hex, 16)
        .map_err(|_| "invalid embedded component manifest length".to_owned())?;
    if !(1..=MAX_EMBEDDED_MANIFEST_JSON_BYTES).contains(&json_len) {
        return Err(format!(
            "embedded component manifest length outside 1..={MAX_EMBEDDED_MANIFEST_JSON_BYTES}"
        ));
    }
    let digest_hex = std::str::from_utf8(&header[prefix_len + 9..102])
        .map_err(|_| "embedded component manifest digest is not ASCII".to_owned())?;
    if !valid_sha256(digest_hex) {
        return Err("embedded component manifest header digest is invalid".into());
    }
    let payload_end = header_end
        .checked_add(json_len)
        .ok_or_else(|| "embedded component manifest payload offset overflow".to_owned())?;
    let trailer_end = payload_end
        .checked_add(trailer_len)
        .ok_or_else(|| "embedded component manifest trailer offset overflow".to_owned())?;
    let json_bytes = bytes
        .get(header_end..payload_end)
        .ok_or_else(|| "truncated embedded component manifest payload".to_owned())?;
    let trailer = bytes
        .get(payload_end..trailer_end)
        .ok_or_else(|| "truncated embedded component manifest frame trailer".to_owned())?;
    if &trailer[..trailer_a.len()] != trailer_a
        || &trailer[trailer_a.len()..] != trailer_b
        || trailer_start != Some(payload_end)
    {
        return Err("embedded component manifest frame trailer mismatch".into());
    }
    let actual_digest = format!("{:x}", Sha256::digest(json_bytes));
    if actual_digest != digest_hex {
        return Err("embedded component manifest payload SHA256 mismatch".into());
    }
    let manifest: EmbeddedAgentManifest = serde_json::from_slice(json_bytes)
        .map_err(|error| format!("invalid embedded component manifest JSON: {error}"))?;
    manifest.validate()?;
    Ok(Some(manifest))
}

pub fn extract_embedded_agent_manifest(bytes: &[u8]) -> Result<EmbeddedAgentManifest, String> {
    extract_embedded_agent_manifest_optional(bytes)?.ok_or_else(|| {
        "signed Agent binary must contain exactly one embedded component manifest frame".into()
    })
}

pub const REQUIRED_WORKBENCH_TOOLS: [&str; 8] = [
    "ping",
    "curl",
    "mtr",
    "iperf3",
    "sysbench",
    "fio",
    "stress-ng",
    "smartctl",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundledManifest {
    pub schema: u32,
    #[serde(alias = "artifact_name")]
    pub component_name: String,
    pub binary_name: String,
    pub version: String,
    pub bundle_sha256: String,
    pub binary_sha256: String,
    pub files: BTreeMap<String, BundledFileRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundledFileRecord {
    pub sha256: String,
    pub size: u64,
    pub executable: bool,
}

pub fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !matches!(value, "." | "..")
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'_' | b'-'))
}

pub fn valid_version_segment(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'_' | b'-'))
}

pub fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Normalizes a relative archive or bundled file path into forward-slash canonical form,
/// rejecting traversal, absolute paths, leading `-` segments, control characters, and
/// paths deeper than `MAX_BUNDLED_PATH_DEPTH`.
pub fn normalize_bundled_relative_path(path: &Path) -> Result<String, String> {
    if path.is_absolute() {
        return Err(format!("bundled path must be relative: {}", path.display()));
    }
    let raw = path
        .to_str()
        .ok_or_else(|| format!("bundled path is not UTF-8: {}", path.display()))?;
    if raw.is_empty()
        || raw.len() > 512
        || raw.contains('\\')
        || raw.starts_with('/')
        || raw.ends_with('/')
        || raw.contains("//")
        || raw.chars().any(char::is_control)
    {
        return Err(format!("unsafe bundled path syntax: {}", path.display()));
    }
    let mut parts = Vec::new();
    for part in raw.split('/') {
        if !valid_segment(part) {
            return Err(format!("invalid bundled path segment '{part}'"));
        }
        parts.push(part);
        if parts.len() > MAX_BUNDLED_PATH_DEPTH {
            return Err(format!(
                "bundled path exceeds depth ceiling ({MAX_BUNDLED_PATH_DEPTH}): {}",
                path.display()
            ));
        }
    }
    if parts.is_empty() {
        return Err("empty bundled relative path".into());
    }
    let normalized = parts.join("/");
    if normalized == BUNDLED_MARKER {
        return Err(format!("reserved bundled marker name '{BUNDLED_MARKER}'"));
    }
    Ok(normalized)
}

impl BundledManifest {
    /// Hashes the canonical file inventory carried by a signed Agent build.
    pub fn inventory_sha256(&self) -> Result<String, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err("bundled manifest schema must equal 1".into());
        }
        if !valid_segment(&self.component_name)
            || !valid_segment(&self.binary_name)
            || !valid_version_segment(&self.version)
            || !valid_sha256(&self.bundle_sha256)
            || !valid_sha256(&self.binary_sha256)
        {
            return Err("invalid bundled manifest header fields".into());
        }
        if self.files.is_empty() || self.files.len() > MAX_BUNDLED_FILES {
            return Err(format!(
                "bundled manifest file count ({}) is outside 1..={MAX_BUNDLED_FILES}",
                self.files.len()
            ));
        }
        let mut total = 0_u64;
        for (name, record) in &self.files {
            let normalized = normalize_bundled_relative_path(Path::new(name))?;
            if &normalized != name {
                return Err(format!("non-canonical bundled file path '{name}'"));
            }
            if !valid_sha256(&record.sha256) {
                return Err(format!("invalid SHA256 for bundled file '{name}'"));
            }
            total = total
                .checked_add(record.size)
                .filter(|sum| *sum <= MAX_BUNDLED_UNPACKED_BYTES)
                .ok_or_else(|| "bundled manifest exceeds unpacked byte ceiling".to_owned())?;
        }
        let binary = self.files.get(&self.binary_name).ok_or_else(|| {
            format!(
                "bundled manifest missing primary binary '{}'",
                self.binary_name
            )
        })?;
        if !binary.executable || binary.sha256 != self.binary_sha256 {
            return Err("bundled primary binary must be executable and match binary_sha256".into());
        }
        Ok(())
    }
}

pub fn validate_binary_abi(
    bytes: &[u8],
    target_os: &str,
    target_arch: &str,
    target_env: &str,
    forbid_glibc_interp: bool,
) -> Result<(), String> {
    match target_os {
        "linux" | "freebsd" => {
            if bytes.len() < 64 || &bytes[0..4] != b"\x7fELF" || bytes[4] != 2 || bytes[5] != 1 {
                return Err(format!(
                    "expected 64-bit little-endian ELF binary for {target_os}/{target_arch}"
                ));
            }
            let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
            let expected_machine = match target_arch {
                "x86_64" => 62,
                "aarch64" => 183,
                other => return Err(format!("unsupported ELF target_arch '{other}'")),
            };
            if machine != expected_machine {
                return Err(format!(
                    "ELF e_machine {machine} does not match target_arch {target_arch}"
                ));
            }
            if target_os == "linux" && target_env == "musl" && forbid_glibc_interp {
                let phoff = usize::try_from(u64::from_le_bytes(
                    bytes[32..40].try_into().map_err(|_| "ELF header")?,
                ))
                .map_err(|_| "ELF phoff overflow")?;
                let phentsize = usize::from(u16::from_le_bytes([bytes[54], bytes[55]]));
                let phnum = usize::from(u16::from_le_bytes([bytes[56], bytes[57]]));
                if phentsize >= 56 && phnum <= 64 {
                    for index in 0..phnum {
                        let Some(offset) = phoff.checked_add(index.saturating_mul(phentsize))
                        else {
                            break;
                        };
                        let Some(header) = bytes.get(offset..offset + 56) else {
                            break;
                        };
                        let p_type =
                            u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
                        if p_type == 3 {
                            let file_off = usize::try_from(u64::from_le_bytes(
                                header[8..16].try_into().map_err(|_| "PT_INTERP")?,
                            ))
                            .unwrap_or(usize::MAX);
                            let file_sz = usize::try_from(u64::from_le_bytes(
                                header[32..40].try_into().map_err(|_| "PT_INTERP")?,
                            ))
                            .unwrap_or(0);
                            if let Some(interp_bytes) =
                                bytes.get(file_off..file_off.saturating_add(file_sz.min(256)))
                            {
                                let interp = String::from_utf8_lossy(interp_bytes);
                                if interp.contains("ld-linux") && !interp.contains("musl") {
                                    return Err(format!(
                                        "musl target cannot embed a glibc-linked ELF binary (PT_INTERP={interp})"
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            Ok(())
        }
        "windows" => {
            if bytes.len() < 0x40 || &bytes[0..2] != b"MZ" {
                return Err("expected Windows PE binary".into());
            }
            let pe = usize::try_from(u32::from_le_bytes([
                bytes[0x3c],
                bytes[0x3d],
                bytes[0x3e],
                bytes[0x3f],
            ]))
            .map_err(|_| "invalid PE offset")?;
            let Some(header) = bytes.get(pe..pe + 6) else {
                return Err("truncated Windows PE header".into());
            };
            if &header[0..4] != b"PE\0\0" {
                return Err("missing PE signature".into());
            }
            let machine = u16::from_le_bytes([header[4], header[5]]);
            let expected = match target_arch {
                "x86_64" => 0x8664,
                "aarch64" => 0xaa64,
                other => return Err(format!("unsupported Windows target_arch '{other}'")),
            };
            if machine != expected {
                return Err(format!(
                    "PE machine {machine:#x} does not match target_arch {target_arch}"
                ));
            }
            Ok(())
        }
        "macos" => {
            if bytes.len() < 8 || bytes[0..4] != [0xcf, 0xfa, 0xed, 0xfe] {
                return Err("expected 64-bit Mach-O binary".into());
            }
            let cputype = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
            let expected = match target_arch {
                "x86_64" => 0x0100_0007,
                "aarch64" => 0x0100_000c,
                other => return Err(format!("unsupported macOS target_arch '{other}'")),
            };
            if cputype != expected {
                return Err(format!(
                    "Mach-O cputype {cputype:#x} does not match target_arch {target_arch}"
                ));
            }
            Ok(())
        }
        other => Err(format!("unsupported target_os '{other}'")),
    }
}

/// Validates component-specific file completeness, executable flags, ABI, and manifest rules
/// shared between compile-time packaging (`crates/agent/build.rs`) and runtime extraction.
pub fn validate_bundled_component_contents<'a, F>(
    manifest: &BundledManifest,
    target_os: &str,
    target_arch: &str,
    target_env: &str,
    mut file_bytes: F,
) -> Result<(), String>
where
    F: FnMut(&str) -> Option<&'a [u8]>,
{
    manifest.validate()?;
    match manifest.component_name.as_str() {
        "sing-box" => {
            let expected_bin = if target_os == "windows" {
                "sing-box.exe"
            } else {
                "sing-box"
            };
            if manifest.binary_name != expected_bin {
                return Err(format!(
                    "sing-box binary_name '{}' does not match target_os '{target_os}'",
                    manifest.binary_name
                ));
            }
            if let Some(bin) = file_bytes(expected_bin) {
                validate_binary_abi(bin, target_os, target_arch, target_env, true)?;
            }
            if manifest.version == "1.14.4-apsp.2" {
                if target_os == "linux" && target_env == "gnu" {
                    if !manifest.files.contains_key("libcronet.so") {
                        return Err(
                            "GNU Linux sing-box 1.14.4-apsp.2 archive must include libcronet.so"
                                .into(),
                        );
                    }
                    if let Some(cronet) = file_bytes("libcronet.so") {
                        validate_binary_abi(cronet, target_os, target_arch, target_env, false)?;
                    }
                } else if target_os == "linux" && target_env == "musl" {
                    if manifest.files.contains_key("libcronet.so") {
                        return Err(
                            "musl Linux sing-box archive must not include glibc libcronet.so"
                                .into(),
                        );
                    }
                } else if target_os == "windows" && !manifest.files.contains_key("libcronet.dll") {
                    return Err(
                        "Windows sing-box 1.14.4-apsp.2 archive must include libcronet.dll".into(),
                    );
                }
            }
        }
        "nodequality" => {
            if manifest.binary_name != "nodequality" {
                return Err("nodequality binary_name must be 'nodequality'".into());
            }
            if manifest.files.contains_key("rootfs.tar.gz")
                && !manifest.files.contains_key("rootfs-manifest.json")
            {
                return Err(
                    "nodequality offline rootfs bundle requires rootfs-manifest.json".into(),
                );
            }
            if manifest.files.contains_key("runtime-manifest.json") {
                for required in ["LICENSE", "THIRD_PARTY_NOTICES.txt"] {
                    if !manifest.files.contains_key(required) {
                        return Err(format!(
                            "self-contained nodequality bundle missing '{required}'"
                        ));
                    }
                }
                if let Some(rt_bytes) = file_bytes("runtime-manifest.json") {
                    let parsed: Value = serde_json::from_slice(rt_bytes).map_err(|error| {
                        format!("invalid nodequality runtime-manifest.json: {error}")
                    })?;
                    if parsed.get("schema").and_then(Value::as_u64) != Some(1)
                        || parsed.get("version").and_then(Value::as_str)
                            != Some(manifest.version.as_str())
                    {
                        return Err(
                            "nodequality runtime-manifest.json schema or version mismatch".into(),
                        );
                    }
                }
                for required_bin in ["bin/bash", "bin/python3", "bin/curl"] {
                    let record = manifest.files.get(required_bin).ok_or_else(|| {
                        format!("self-contained nodequality bundle missing '{required_bin}'")
                    })?;
                    if !record.executable {
                        return Err(format!(
                            "self-contained nodequality helper '{required_bin}' must be executable"
                        ));
                    }
                    if let Some(bin_bytes) = file_bytes(required_bin) {
                        validate_binary_abi(bin_bytes, target_os, target_arch, target_env, false)?;
                    }
                }
            }
        }
        "ipquality" => {
            for required in [
                "ipquality",
                "rootfs.tar.gz",
                "rootfs-manifest.json",
                "build-info.json",
                "LICENSE",
                "source.tar.gz",
                "THIRD_PARTY_NOTICES.txt",
            ] {
                if !manifest.files.contains_key(required) {
                    return Err(format!(
                        "ipquality archive must be a complete offline rootfs bundle; missing '{required}'"
                    ));
                }
            }
            if let Some(record) = manifest.files.get("bin/python3") {
                if !record.executable {
                    return Err(
                        "self-contained ipquality helper 'bin/python3' must be executable".into(),
                    );
                }
                if let Some(bin_bytes) = file_bytes("bin/python3") {
                    validate_binary_abi(bin_bytes, target_os, target_arch, target_env, false)?;
                }
            }
        }
        "network-workbench" => {
            for required in ["apsp-network-workbench", "tools-manifest.json", "LICENSE"] {
                if !manifest.files.contains_key(required) {
                    return Err(format!("network-workbench archive is missing '{required}'"));
                }
            }
            if let Some(manifest_bytes) = file_bytes("tools-manifest.json") {
                let parsed: Value = serde_json::from_slice(manifest_bytes).map_err(|error| {
                    format!("invalid network-workbench tools-manifest.json: {error}")
                })?;
                if parsed.get("schema").and_then(Value::as_u64) != Some(1) {
                    return Err("network-workbench tools-manifest.json schema must equal 1".into());
                }
                let interp_raw = parsed
                    .get("interpreter")
                    .and_then(|v| v.get("bundled_path"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        "network-workbench requires interpreter.bundled_path".to_owned()
                    })?;
                let interp = normalize_bundled_relative_path(Path::new(interp_raw))?;
                let interp_rec = manifest.files.get(&interp).ok_or_else(|| {
                    format!("network-workbench interpreter '{interp}' missing from archive")
                })?;
                if !interp_rec.executable {
                    return Err(format!(
                        "network-workbench interpreter '{interp}' must be executable"
                    ));
                }
                let tools = parsed
                    .get("tools")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        "network-workbench tools-manifest.json missing tools".to_owned()
                    })?;
                for required_tool in REQUIRED_WORKBENCH_TOOLS {
                    if !tools.contains_key(required_tool) {
                        return Err(format!(
                            "network-workbench bundle is incomplete; missing required tool '{required_tool}'"
                        ));
                    }
                }
                if !tools.contains_key("nexttrace") && !tools.contains_key("traceroute") {
                    return Err(
                        "network-workbench bundle requires 'nexttrace' or 'traceroute'".into(),
                    );
                }
                for (tool_name, entry) in tools {
                    if entry.get("licensed").and_then(Value::as_bool) != Some(true) {
                        return Err(format!(
                            "network-workbench tool '{tool_name}' lacks license confirmation"
                        ));
                    }
                    let raw_tool_path = entry
                        .get("bundled_path")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            format!(
                                "network-workbench tool '{tool_name}' must declare relative bundled_path"
                            )
                        })?;
                    let tool_path = normalize_bundled_relative_path(Path::new(raw_tool_path))?;
                    let record = manifest.files.get(&tool_path).ok_or_else(|| {
                        format!("network-workbench tool '{tool_name}' missing at '{tool_path}'")
                    })?;
                    if !record.executable {
                        return Err(format!(
                            "network-workbench tool '{tool_name}' at '{tool_path}' must be executable"
                        ));
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_agent_manifest_frame_ignores_unrelated_hex64_and_derives_subcommand_digest() {
        let singbox_sha = "1".repeat(64);
        let ipquality_sha = "2".repeat(64);
        let agent_sha = "9".repeat(64);
        let unrelated_hex = "a".repeat(64);
        let manifest = EmbeddedAgentManifest {
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
                        inventory_sha256: None,
                        sha256: Some(singbox_sha.clone()),
                        subcommand: None,
                        version: "1.14.4-apsp.2".into(),
                    },
                ),
                (
                    "ipquality".into(),
                    EmbeddedComponentRecord {
                        format: "tar.gz".into(),
                        inventory_sha256: None,
                        sha256: Some(ipquality_sha.clone()),
                        subcommand: None,
                        version: "87397e2c3196ec796f5477c83343c2354df601ea-node-r1".into(),
                    },
                ),
                (
                    "tcpquality".into(),
                    EmbeddedComponentRecord {
                        format: "agent_subcommand".into(),
                        inventory_sha256: None,
                        sha256: None,
                        subcommand: Some("tcp-probe".into()),
                        version: format!("0.3.0-{}-r1", "b".repeat(40)),
                    },
                ),
            ]),
        };
        let frame = encode_embedded_agent_manifest_frame(&manifest).unwrap();
        let mut binary = b"\x7fELF\x02\x01fixture-rodata:".to_vec();
        binary.extend_from_slice(unrelated_hex.as_bytes());
        binary.extend_from_slice(b":middle:");
        binary.extend_from_slice(&frame);
        binary.extend_from_slice(b":trailing");

        let parsed = extract_embedded_agent_manifest(&binary).unwrap();
        assert_eq!(parsed, manifest);
        assert!(
            !parsed
                .components
                .values()
                .any(|entry| { entry.sha256.as_deref() == Some(unrelated_hex.as_str()) })
        );
        assert_eq!(
            expected_embedded_component_digest(
                "sing-box",
                &parsed.components["sing-box"],
                &agent_sha
            ),
            Some(singbox_sha.as_str())
        );
        assert_eq!(
            expected_embedded_component_digest(
                "tcpquality",
                &parsed.components["tcpquality"],
                &agent_sha
            ),
            Some(agent_sha.as_str())
        );

        let mut duplicate = binary.clone();
        duplicate.extend_from_slice(&frame);
        assert!(extract_embedded_agent_manifest(&duplicate).is_err());

        let (trailer_a, trailer_b) = frame_trailer_parts();
        let mut raw_trailer = Vec::new();
        raw_trailer.extend_from_slice(trailer_a);
        raw_trailer.extend_from_slice(trailer_b);
        let mut orphan_trailer = b"\x7fELF\x02\x01orphan".to_vec();
        orphan_trailer.extend_from_slice(&raw_trailer);
        assert!(extract_embedded_agent_manifest_optional(&orphan_trailer).is_err());
        assert!(extract_embedded_agent_manifest(&orphan_trailer).is_err());

        let mut leading_extra_trailer = raw_trailer.clone();
        leading_extra_trailer.extend_from_slice(&binary);
        assert!(extract_embedded_agent_manifest_optional(&leading_extra_trailer).is_err());

        let mut trailing_extra_trailer = binary.clone();
        trailing_extra_trailer.extend_from_slice(&raw_trailer);
        assert!(extract_embedded_agent_manifest_optional(&trailing_extra_trailer).is_err());

        let mut overlapping_extra_trailer = raw_trailer[..raw_trailer.len() - 1].to_vec();
        overlapping_extra_trailer.extend_from_slice(&frame);
        assert!(extract_embedded_agent_manifest_optional(&overlapping_extra_trailer).is_err());

        assert_eq!(
            extract_embedded_agent_manifest_optional(unrelated_hex.as_bytes()).unwrap(),
            None
        );
    }

    #[test]
    fn bundled_manifest_serializes_component_name_and_accepts_legacy_artifact_name_alias() {
        let bin_sha = "3".repeat(64);
        let bundle_sha = "4".repeat(64);
        let manifest = BundledManifest {
            schema: 1,
            component_name: "sing-box".into(),
            binary_name: "sing-box".into(),
            version: "1.14.4-apsp.2".into(),
            bundle_sha256: bundle_sha.clone(),
            binary_sha256: bin_sha.clone(),
            files: BTreeMap::from([(
                "sing-box".into(),
                BundledFileRecord {
                    sha256: bin_sha.clone(),
                    size: 16,
                    executable: true,
                },
            )]),
        };
        let encoded = serde_json::to_string(&manifest).unwrap();
        assert!(encoded.contains("\"component_name\":\"sing-box\""));
        assert!(!encoded.contains("\"artifact_name\""));
        let decoded: BundledManifest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, manifest);
        decoded.validate().unwrap();

        let legacy = encoded.replace("\"component_name\":", "\"artifact_name\":");
        let from_legacy: BundledManifest = serde_json::from_str(&legacy).unwrap();
        assert_eq!(from_legacy, manifest);
        from_legacy.validate().unwrap();
    }
}

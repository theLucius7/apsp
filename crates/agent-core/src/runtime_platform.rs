use apsp_protocol::release::{
    ReleaseError, VerifiedArtifact, VerifiedRelease, native_arch, native_target,
};
use std::{fs::File, io::Read};

const ELF_PREFIX_LIMIT: u64 = 64 * 1024;

/// Inspect the host shell instead of assuming a static Agent uses the host libc.
/// Unknown or static shells retain the Agent's original ABI selection.
pub(crate) fn libc() -> Option<&'static str> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let compiled = if cfg!(target_env = "musl") {
        "musl"
    } else {
        "gnu"
    };
    let detected = (|| {
        let mut bytes = Vec::new();
        File::open("/bin/sh")
            .ok()?
            .take(ELF_PREFIX_LIMIT)
            .read_to_end(&mut bytes)
            .ok()?;
        interpreter_libc(&bytes)
    })();
    Some(detected.unwrap_or(compiled))
}

fn interpreter_libc(bytes: &[u8]) -> Option<&'static str> {
    // Supported Linux Agent targets are little-endian ELF64 (amd64 and arm64).
    if bytes.get(..7)? != b"\x7fELF\x02\x01\x01" {
        return None;
    }
    let u16_at = |offset| {
        Some(u16::from_le_bytes(
            bytes.get(offset..offset + 2)?.try_into().ok()?,
        ))
    };
    let u32_at = |offset| {
        Some(u32::from_le_bytes(
            bytes.get(offset..offset + 4)?.try_into().ok()?,
        ))
    };
    let u64_at = |offset| {
        usize::try_from(u64::from_le_bytes(
            bytes.get(offset..offset + 8)?.try_into().ok()?,
        ))
        .ok()
    };
    let table = u64_at(32)?;
    let size = usize::from(u16_at(54)?);
    let count = usize::from(u16_at(56)?);
    if size != 56 || count == 0 || count > 1024 {
        return None;
    }
    bytes.get(table..table.checked_add(size.checked_mul(count)?)?)?;
    let mut interpreter = None;
    for index in 0..count {
        let header = table.checked_add(index.checked_mul(size)?)?;
        if u32_at(header)? != 3 {
            continue;
        }
        if interpreter.is_some() {
            return None;
        }
        let offset = u64_at(header + 8)?;
        let length = u64_at(header + 32)?;
        if !(2..=4096).contains(&length) {
            return None;
        }
        let path = bytes.get(offset..offset.checked_add(length)?)?;
        let (terminator, path) = path.split_last()?;
        if *terminator != 0 || path.contains(&0) || path.first() != Some(&b'/') {
            return None;
        }
        interpreter = Some(std::str::from_utf8(path).ok()?);
    }
    match interpreter?.rsplit('/').next()? {
        "ld-linux-x86-64.so.2" | "ld-linux-aarch64.so.1" => Some("gnu"),
        "ld-musl-x86_64.so.1" | "ld-musl-aarch64.so.1" => Some("musl"),
        _ => None,
    }
}

pub(crate) fn artifact(
    release: &VerifiedRelease,
    name: &str,
    version: &str,
) -> Result<VerifiedArtifact, ReleaseError> {
    let target = apsp_protocol::platform::artifact_target(
        std::env::consts::OS,
        libc(),
        std::env::consts::ARCH,
    )
    .ok_or(ReleaseError::IdentityMismatch)?;
    artifact_for_targets(release, name, version, &target, &native_target()?)
}

fn artifact_for_targets(
    release: &VerifiedRelease,
    name: &str,
    version: &str,
    runtime_target: &str,
    compiled_target: &str,
) -> Result<VerifiedArtifact, ReleaseError> {
    if name == "agent" {
        // Self-updates remain bound to the ABI of this Agent executable.
        return release.native_artifact(name, version);
    }
    // Preserve the previous identity of signed Linux caches when the host ABI
    // differs, including GNU Agents already running on a musl compatibility
    // layer. A new host target follows both identities known by the old Agent.
    let different_linux_abis = (compiled_target.starts_with("linux-musl-")
        && runtime_target.starts_with("linux-gnu-"))
        || (compiled_target.starts_with("linux-gnu-") && runtime_target.starts_with("linux-musl-"));
    if different_linux_abis {
        for target in [compiled_target, native_arch()?] {
            match release.artifact(name, version, target) {
                Ok(artifact) => return Ok(artifact),
                Err(ReleaseError::MissingArtifact) => {}
                Err(error) => return Err(error),
            }
        }
        return release.artifact(name, version, runtime_target);
    }
    match release.artifact(name, version, runtime_target) {
        Ok(artifact) => Ok(artifact),
        // Preserve the existing signed, architecture-only artifact format.
        // The panel decides which runtime ABI it can safely offer this host.
        Err(ReleaseError::MissingArtifact) => release.artifact(name, version, native_arch()?),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release_test_support as support;
    use apsp_protocol::release::{canonical_asset_name, verify_release};

    fn elf(interpreter: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 120];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[32..40].copy_from_slice(&64_u64.to_le_bytes());
        bytes[54..56].copy_from_slice(&56_u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&1_u16.to_le_bytes());
        bytes[64..68].copy_from_slice(&3_u32.to_le_bytes());
        bytes[72..80].copy_from_slice(&120_u64.to_le_bytes());
        bytes[96..104].copy_from_slice(&(interpreter.len() as u64).to_le_bytes());
        bytes.extend_from_slice(interpreter);
        bytes
    }

    #[test]
    fn host_interpreter_distinguishes_gnu_and_musl_on_both_architectures() {
        for (path, expected) in [
            ("/lib64/ld-linux-x86-64.so.2\0", "gnu"),
            ("/lib/ld-linux-aarch64.so.1\0", "gnu"),
            ("/lib/ld-musl-x86_64.so.1\0", "musl"),
            ("/lib/ld-musl-aarch64.so.1\0", "musl"),
        ] {
            assert_eq!(interpreter_libc(&elf(path.as_bytes())), Some(expected));
        }
    }

    #[test]
    fn unknown_static_and_malformed_shells_do_not_claim_a_host_abi() {
        for value in [
            b"not ELF".to_vec(),
            elf(b"/unknown/loader\0"),
            elf(b"/lib/ld-musl-x86_64.so.1"),
            elf(b"/lib/ld-musl-x86_64.so.1\0hidden\0"),
            elf(b"ld-linux-x86-64.so.2\0"),
        ] {
            assert_eq!(interpreter_libc(&value), None);
        }
        let good = elf(b"/lib64/ld-linux-x86-64.so.2\0");
        for end in 0..good.len() {
            assert_eq!(interpreter_libc(&good[..end]), None);
        }
        for (offset, value) in [(32, u64::MAX), (72, u64::MAX), (96, u64::MAX)] {
            let mut bytes = good.clone();
            bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            assert_eq!(interpreter_libc(&bytes), None);
        }
        let mut static_shell = good;
        static_shell[64..68].copy_from_slice(&1_u32.to_le_bytes());
        assert_eq!(interpreter_libc(&static_shell), None);
    }

    #[test]
    fn runtime_selection_preserves_musl_caches_and_agent_and_legacy_identities() {
        let arch = native_arch().unwrap();
        let gnu = format!("linux-gnu-{arch}");
        let musl = format!("linux-musl-{arch}");
        let agent_target = native_target().unwrap();
        let mut artifacts = Vec::new();
        for (name, target) in [
            ("runtime", gnu.as_str()),
            ("runtime", musl.as_str()),
            ("gnu-only-runtime", gnu.as_str()),
            ("musl-only-runtime", musl.as_str()),
            ("gnu-and-legacy", gnu.as_str()),
            ("gnu-and-legacy", arch),
            ("musl-and-legacy", musl.as_str()),
            ("musl-and-legacy", arch),
            ("agent", agent_target.as_str()),
            ("legacy-plugin", arch),
        ] {
            let format = if name == "agent" { "raw" } else { "tar.gz" };
            let mut entry = support::entry(name, "0.3.0", name, format, b"bytes", b"bytes");
            entry.arch = target.into();
            if name == "agent" {
                entry.binary_name = crate::system::deploy::executable_name().into();
            }
            entry.asset_name = canonical_asset_name(&entry).unwrap();
            artifacts.push((entry, b"bytes".to_vec()));
        }
        let proof = support::signed_release(artifacts);
        let release = verify_release(&proof, &support::trusted_keys()).unwrap();
        for (target, compiled, expected) in [
            (&gnu, &gnu, &gnu),
            (&gnu, &musl, &musl),
            (&musl, &musl, &musl),
            (&musl, &gnu, &gnu),
        ] {
            assert_eq!(
                artifact_for_targets(&release, "runtime", "0.3.0", target, compiled)
                    .unwrap()
                    .metadata()
                    .arch,
                *expected
            );
            assert_eq!(
                artifact_for_targets(&release, "agent", "0.3.0", target, compiled)
                    .unwrap()
                    .metadata()
                    .arch,
                native_target().unwrap()
            );
            assert_eq!(
                artifact_for_targets(&release, "legacy-plugin", "0.3.0", target, compiled)
                    .unwrap()
                    .metadata()
                    .arch,
                arch
            );
        }
        assert_eq!(
            artifact_for_targets(&release, "gnu-only-runtime", "0.3.0", &gnu, &musl)
                .unwrap()
                .metadata()
                .arch,
            gnu
        );
        assert!(artifact_for_targets(&release, "gnu-only-runtime", "0.3.0", &musl, &musl).is_err());
        assert_eq!(
            artifact_for_targets(&release, "gnu-only-runtime", "0.3.0", &musl, &gnu)
                .unwrap()
                .metadata()
                .arch,
            gnu
        );
        assert_eq!(
            artifact_for_targets(&release, "musl-only-runtime", "0.3.0", &musl, &gnu)
                .unwrap()
                .metadata()
                .arch,
            musl
        );
        for (name, expected) in [("gnu-and-legacy", arch), ("musl-and-legacy", musl.as_str())] {
            assert_eq!(
                artifact_for_targets(&release, name, "0.3.0", &gnu, &musl)
                    .unwrap()
                    .metadata()
                    .arch,
                expected
            );
        }
        for (name, expected) in [("gnu-and-legacy", gnu.as_str()), ("musl-and-legacy", arch)] {
            assert_eq!(
                artifact_for_targets(&release, name, "0.3.0", &musl, &gnu)
                    .unwrap()
                    .metadata()
                    .arch,
                expected
            );
        }
    }
}

#![forbid(unsafe_code)]

#[path = "support/release.rs"]
mod support;
use apsp_protocol::release::{ReleaseMetadata, TrustedKeys, verify_release};
use base64::{Engine, engine::general_purpose::STANDARD};

fn proof() -> apsp_protocol::release::ReleaseProof {
    let bytes = b"test executable";
    support::signed_release(vec![(
        support::entry("agent", "0.3.0", "apsp-agent", "raw", bytes, bytes),
        bytes.to_vec(),
    )])
}

#[test]
fn test_signer_matches_real_minisign_cli_fixture() {
    assert_eq!(
        support::sign(include_bytes!("fixtures/payload.txt")),
        include_str!("fixtures/payload.txt.minisig")
    );
}

#[test]
fn verifies_signed_identities_and_rejects_tampering() {
    let proof = proof();
    let verified = verify_release(&proof, &support::trusted_keys()).unwrap();
    let artifact = verified
        .artifact(
            "agent",
            "0.3.0",
            apsp_protocol::release::native_arch().unwrap(),
        )
        .unwrap();
    artifact.verify_archive(b"test executable").unwrap();
    artifact.verify_binary(b"test executable").unwrap();
    assert!(artifact.verify_binary(b"changed executable").is_err());
    assert!(verified.artifact("agent", "0.2.0", "amd64").is_err());
    for mutate in 0..4 {
        let mut invalid = proof.clone();
        match mutate {
            0 => invalid.checksums.push('\n'),
            1 => invalid.metadata_json.push(' '),
            2 => invalid.signature = invalid.signature.replace("never trust", "always trust"),
            _ => invalid.signature.push_str("hidden line\n"),
        }
        assert!(verify_release(&invalid, &support::trusted_keys()).is_err());
    }
}

#[test]
fn rejects_signed_noncanonical_metadata_and_checksums() {
    let mut proof = proof();
    for checksums in [
        proof.checksums.replace('\n', "\r\n"),
        proof.checksums.repeat(2),
    ] {
        let mut invalid = proof.clone();
        invalid.signature = support::sign(checksums.as_bytes());
        invalid.checksums = checksums;
        assert!(verify_release(&invalid, &support::trusted_keys()).is_err());
    }
    let mut metadata: ReleaseMetadata = serde_json::from_str(&proof.metadata_json).unwrap();
    metadata.artifacts[0].asset_name = "../wrong".into();
    proof.metadata_json = serde_json::to_string(&metadata).unwrap();
    proof.checksums = proof
        .checksums
        .lines()
        .map(|line| {
            if line.ends_with("  release.json") {
                format!(
                    "{}  release.json\n",
                    support::hash(proof.metadata_json.as_bytes())
                )
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    proof.signature = support::sign(proof.checksums.as_bytes());
    assert!(verify_release(&proof, &support::trusted_keys()).is_err());
}

#[test]
fn explicit_roots_allow_rotation_but_reject_wrong_and_duplicate_keys() {
    let encoded = support::PUBLIC_KEY.lines().nth(1).unwrap();
    let other = support::rotation_public_key();
    let old = TrustedKeys::from_json(&serde_json::to_string(&[encoded]).unwrap()).unwrap();
    let new = TrustedKeys::from_json(&serde_json::to_string(&[&other]).unwrap()).unwrap();
    let both = TrustedKeys::from_json(&serde_json::to_string(&[&other, encoded]).unwrap()).unwrap();
    let previous_release = proof();
    verify_release(&previous_release, &old).unwrap();
    assert!(verify_release(&previous_release, &new).is_err());
    verify_release(&previous_release, &both).unwrap();
    let mut next_release = previous_release.clone();
    next_release.signature = support::rotation_sign(next_release.checksums.as_bytes());
    assert!(verify_release(&next_release, &old).is_err());
    verify_release(&next_release, &both).unwrap();
    verify_release(&next_release, &new).unwrap();
    assert!(TrustedKeys::from_json(&serde_json::to_string(&[encoded, encoded]).unwrap()).is_err());
    assert!(TrustedKeys::from_json("[]").is_err());
    let mut duplicate_different_id = STANDARD.decode(encoded).unwrap();
    duplicate_different_id[2] ^= 1;
    assert!(
        TrustedKeys::from_json(
            &serde_json::to_string(&[encoded, &STANDARD.encode(duplicate_different_id)]).unwrap()
        )
        .is_err()
    );
    let expected = format!(
        "untrusted comment: Apsp TEST ONLY rotation key; public deterministic seed\n{}\n",
        other
    );
    assert_eq!(expected, include_str!("fixtures/TEST_ONLY_ROTATION.pub"));
}

#[test]
fn signed_platform_identities_preserve_canonical_asset_names() {
    use apsp_protocol::release::{canonical_asset_name, canonical_path};
    let bytes = b"platform executable";
    for target in apsp_protocol::platform::ARTIFACT_TARGETS {
        let binary_name = if target.starts_with("windows-") {
            "apsp-agent.exe"
        } else {
            "apsp-agent"
        };
        let mut entry = support::entry("agent", "0.3.0", binary_name, "raw", bytes, bytes);
        entry.arch = (*target).into();
        entry.asset_name = canonical_asset_name(&entry).unwrap();
        let proof = support::signed_release(vec![(entry.clone(), bytes.to_vec())]);
        let verified = verify_release(&proof, &support::trusted_keys()).unwrap();
        let artifact = verified.artifact("agent", "0.3.0", target).unwrap();
        artifact.verify_binary(bytes).unwrap();
        assert_eq!(
            artifact.path(),
            canonical_path("agent", "0.3.0", target).unwrap()
        );
        assert_eq!(artifact.metadata().asset_name, entry.asset_name);
        assert!(verified.artifact("agent", "0.3.0", "riscv64").is_err());
    }
}

#[test]
fn signed_auxiliary_metadata_rejects_reserved_names_unbounded_lists_and_raw_files() {
    use apsp_protocol::release::{ReleaseFile, canonical_asset_name};
    let bytes = b"signed archive fixture";
    let valid_file = ReleaseFile {
        sha256: support::hash(b"DLL"),
        size: 3,
    };
    let mut entry = support::entry(
        "sing-box",
        "1.14.2",
        "sing-box.exe",
        "tar.gz",
        bytes,
        b"binary",
    );
    entry.arch = "windows-amd64".into();
    entry.asset_name = canonical_asset_name(&entry).unwrap();
    entry
        .auxiliary_files
        .insert("wintun.dll".into(), valid_file.clone());
    let valid = support::signed_release(vec![(entry.clone(), bytes.to_vec())]);
    let verified = verify_release(&valid, &support::trusted_keys()).unwrap();
    assert_eq!(
        verified
            .artifact("sing-box", "1.14.2", "windows-amd64")
            .unwrap()
            .metadata()
            .auxiliary_files["wintun.dll"],
        valid_file
    );
    for name in [
        "../wintun.dll",
        "sing-box.exe",
        "release.json",
        "SHA256SUMS",
        "SHA256SUMS.minisig",
        ".artifact.json",
    ] {
        let mut invalid = entry.clone();
        invalid.auxiliary_files.clear();
        invalid
            .auxiliary_files
            .insert(name.into(), valid_file.clone());
        let proof = support::signed_release(vec![(invalid, bytes.to_vec())]);
        assert!(verify_release(&proof, &support::trusted_keys()).is_err());
    }
    for file in [
        ReleaseFile {
            size: 0,
            ..valid_file.clone()
        },
        ReleaseFile {
            size: 256 * 1024 * 1024 + 1,
            ..valid_file.clone()
        },
        ReleaseFile {
            sha256: "g".repeat(64),
            ..valid_file.clone()
        },
    ] {
        let mut invalid = entry.clone();
        invalid.auxiliary_files.insert("wintun.dll".into(), file);
        let proof = support::signed_release(vec![(invalid, bytes.to_vec())]);
        assert!(verify_release(&proof, &support::trusted_keys()).is_err());
    }
    let mut invalid = entry.clone();
    invalid.auxiliary_files = (0..8)
        .map(|number| (format!("extra-{number}.dll"), valid_file.clone()))
        .collect();
    assert!(
        verify_release(
            &support::signed_release(vec![(invalid, bytes.to_vec())]),
            &support::trusted_keys()
        )
        .is_err()
    );
    let mut raw = support::entry("agent", "0.3.0", "apsp-agent", "raw", bytes, bytes);
    raw.auxiliary_files.insert("wintun.dll".into(), valid_file);
    assert!(
        verify_release(
            &support::signed_release(vec![(raw, bytes.to_vec())]),
            &support::trusted_keys()
        )
        .is_err()
    );
    let mut tampered = valid;
    tampered.metadata_json = tampered
        .metadata_json
        .replace("wintun.dll", "untrusted.dll");
    assert!(verify_release(&tampered, &support::trusted_keys()).is_err());
}

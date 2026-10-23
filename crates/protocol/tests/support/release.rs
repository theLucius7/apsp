#![forbid(unsafe_code)]
#![allow(dead_code)]

use apsp_protocol::release::{
    ReleaseArtifact, ReleaseMetadata, ReleaseProof, TrustedKeys, canonical_asset_name, native_arch,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use blake2::{Blake2b512, Digest};
use ed25519_dalek::{Signer, SigningKey};
use sha2::Sha256;
use std::{collections::BTreeMap, path::Path};

// This unencrypted key is deliberately public test material, never a production root.
pub const PUBLIC_KEYS: &str = include_str!("../fixtures/public-keys.json");
pub const PUBLIC_KEY: &str = include_str!("../fixtures/TEST_ONLY.pub");
const PRIVATE_KEY: &str = include_str!("../fixtures/TEST_ONLY.key");

pub fn trusted_keys() -> TrustedKeys {
    TrustedKeys::from_json(PUBLIC_KEYS).unwrap()
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn sign(bytes: &[u8]) -> String {
    let private = STANDARD
        .decode(PRIVATE_KEY.lines().nth(1).unwrap())
        .unwrap();
    assert_eq!(private.len(), 158);
    assert_eq!(&private[2..4], &[0, 0]);
    let signing = SigningKey::from_bytes(private[62..94].try_into().unwrap());
    let public = STANDARD.decode(PUBLIC_KEY.lines().nth(1).unwrap()).unwrap();
    assert_eq!(&private[54..62], &public[2..10]);
    assert_eq!(signing.verifying_key().as_bytes(), &public[10..42]);
    sign_with_key(bytes, &signing, public[2..10].try_into().unwrap())
}

// This deterministic seed is public test material used only to exercise root rotation.
pub fn rotation_public_key() -> String {
    let signing = SigningKey::from_bytes(&[0x99; 32]);
    let mut public = b"Ed".to_vec();
    public.extend_from_slice(b"TESTROT2");
    public.extend_from_slice(signing.verifying_key().as_bytes());
    STANDARD.encode(public)
}

pub fn rotation_sign(bytes: &[u8]) -> String {
    sign_with_key(bytes, &SigningKey::from_bytes(&[0x99; 32]), b"TESTROT2")
}

fn sign_with_key(bytes: &[u8], signing: &SigningKey, key_id: &[u8; 8]) -> String {
    let signature = signing.sign(&Blake2b512::digest(bytes));
    let comment = "Apsp TEST ONLY fixture; never trust for official releases";
    let mut record = b"ED".to_vec();
    record.extend_from_slice(key_id);
    record.extend_from_slice(&signature.to_bytes());
    let mut global = signature.to_bytes().to_vec();
    global.extend_from_slice(comment.as_bytes());
    let global_signature = signing.sign(&global);
    format!(
        "untrusted comment: signature from minisign secret key\n{}\ntrusted comment: {comment}\n{}\n",
        STANDARD.encode(record),
        STANDARD.encode(global_signature.to_bytes())
    )
}

pub fn entry(
    name: &str,
    version: &str,
    binary_name: &str,
    format: &str,
    archive: &[u8],
    binary: &[u8],
) -> ReleaseArtifact {
    let mut value = ReleaseArtifact {
        name: name.into(),
        version: version.into(),
        arch: native_arch().unwrap().into(),
        format: format.into(),
        binary_name: binary_name.into(),
        archive_size: archive.len() as u64,
        binary_sha256: hash(binary),
        binary_size: binary.len() as u64,
        asset_name: String::new(),
        auxiliary_files: BTreeMap::new(),
    };
    value.asset_name = canonical_asset_name(&value).unwrap();
    value
}

pub fn signed_release(artifacts: Vec<(ReleaseArtifact, Vec<u8>)>) -> ReleaseProof {
    let metadata = ReleaseMetadata {
        schema: 1,
        source_repo: "theLucius7/apsp".into(),
        tag: "agent-v0.3.0".into(),
        protocol_min: 1,
        protocol_max: 1,
        artifacts: artifacts.iter().map(|(entry, _)| entry.clone()).collect(),
    };
    let metadata_json = format!("{}\n", serde_json::to_string(&metadata).unwrap());
    let mut sums = BTreeMap::from([
        ("release.json".into(), hash(metadata_json.as_bytes())),
        ("install.sh".into(), hash(b"#!/bin/sh\nexit 0\n")),
    ]);
    for (entry, bytes) in artifacts {
        sums.insert(
            format!("{}/{}/{}", entry.name, entry.version, entry.arch),
            hash(&bytes),
        );
    }
    let checksums: String = sums
        .into_iter()
        .map(|(path, hash)| format!("{hash}  {path}\n"))
        .collect();
    let signature = sign(checksums.as_bytes());
    ReleaseProof {
        metadata_json,
        checksums,
        signature,
    }
}

pub fn proof_for_archive(
    name: &str,
    version: &str,
    binary_name: &str,
    archive: &[u8],
    binary: &[u8],
) -> ReleaseProof {
    signed_release(vec![(
        entry(name, version, binary_name, "tar.gz", archive, binary),
        archive.to_vec(),
    )])
}

pub fn install_proof(directory: &Path, proof: &ReleaseProof) {
    std::fs::write(directory.join("release.json"), &proof.metadata_json).unwrap();
    std::fs::write(directory.join("SHA256SUMS"), &proof.checksums).unwrap();
    std::fs::write(directory.join("SHA256SUMS.minisig"), &proof.signature).unwrap();
}

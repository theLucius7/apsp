#![forbid(unsafe_code)]

mod release_fixture;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
#[path = "releases/targets.rs"]
mod targets;
#[path = "releases/versions.rs"]
mod versions;

use anyhow::{Context, Result, bail};
use apsp_governance::releases;
use apsp_panel_host::{AppState, config::Config, error::ApiError};
use apsp_protocol::release::{ReleaseMetadata, ReleaseProof, native_arch};
use release_support as signing;
use sqlx::postgres::PgPoolOptions;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Semaphore;
use uuid::Uuid;

struct Fixture {
    root: PathBuf,
    state: AppState,
}

impl Fixture {
    fn new() -> Result<Self> {
        let root = std::env::temp_dir()
            .canonicalize()?
            .join(format!("apsp-release-test-{}", Uuid::new_v4()));
        std::fs::create_dir(&root)?;
        let data = root.join("data");
        std::fs::create_dir(&data)?;
        let state = AppState {
            started_at: apsp_protocol::now_timestamp(),
            pool: PgPoolOptions::new().connect_lazy("postgres://fixture@127.0.0.1/unused")?,
            login_permits: Arc::new(Semaphore::new(4)),
            quality_permits: Arc::new(Semaphore::new(2)),
            domain_states: Default::default(),
            release_permits: Arc::new(Semaphore::new(1)),
            release_keys: Some(Arc::new(signing::trusted_keys())),
            config: Arc::new(Config {
                database_url: "postgres://fixture@127.0.0.1/unused".into(),
                listen: "127.0.0.1:0".parse()?,
                public_url: "http://127.0.0.1".into(),
                data_dir: data,
                admin_password: None,
            }),
        };
        Ok(Self { root, state })
    }

    fn release_root(&self) -> PathBuf {
        self.state.config.data_dir.join("artifacts/releases")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone)]
struct Bundle {
    tag: String,
    proof: ReleaseProof,
    assets: BTreeMap<String, Vec<u8>>,
}

fn bundle(
    version: &str,
    protocol: (u16, u16),
    agent: &[u8],
    runtime: Option<&[u8]>,
) -> Result<Bundle> {
    let mut artifacts = vec![(
        signing::entry("agent", version, "apsp-agent", "raw", agent, agent),
        agent.to_vec(),
    )];
    if let Some(binary) = runtime {
        let archive = release_fixture::archive("sing-box", binary)?;
        artifacts.push((
            signing::entry(
                "sing-box",
                "1.14.4-apsp.2",
                "sing-box",
                "tar.gz",
                &archive,
                binary,
            ),
            archive,
        ));
    }
    let tag = format!("agent-v{version}");
    let mut proof = signing::signed_release(artifacts.clone());
    let mut metadata: ReleaseMetadata = serde_json::from_str(&proof.metadata_json)?;
    metadata.tag = tag.clone();
    metadata.protocol_min = protocol.0;
    metadata.protocol_max = protocol.1;
    proof.metadata_json = format!("{}\n", serde_json::to_string(&metadata)?);
    proof.checksums = proof
        .checksums
        .lines()
        .map(|line| {
            if line.ends_with("  release.json") {
                format!(
                    "{}  release.json\n",
                    signing::hash(proof.metadata_json.as_bytes())
                )
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    proof.signature = signing::sign(proof.checksums.as_bytes());
    let assets = artifacts
        .into_iter()
        .map(|(entry, bytes)| (entry.asset_name, bytes))
        .chain(std::iter::once((
            "install.sh".into(),
            b"#!/bin/sh\nexit 0\n".to_vec(),
        )))
        .collect();
    Ok(Bundle { tag, proof, assets })
}

async fn import(state: &AppState, bundle: Bundle) -> Result<usize, ApiError> {
    // These signed fixtures contain legacy Linux artifacts for the native CPU,
    // independently of the platform running the test process.
    releases::import_bundle_for_targets(
        state,
        &bundle.tag,
        bundle.proof,
        &[native_arch().unwrap().into()],
        move |name, maximum| {
            let bytes = bundle
                .assets
                .get(&name)
                .cloned()
                .context("fixture asset is missing");
            async move {
                let bytes = bytes?;
                anyhow::ensure!(bytes.len() <= maximum, "fixture exceeds download bound");
                Ok(bytes)
            }
        },
    )
    .await
}

fn snapshot(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) -> Result<()> {
        for entry in std::fs::read_dir(path)? {
            let path = entry?.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                visit(root, &path, result)?;
            } else if metadata.is_file() {
                result.insert(path.strip_prefix(root)?.to_owned(), std::fs::read(path)?);
            }
        }
        Ok(())
    }
    let mut result = BTreeMap::new();
    if root.exists() {
        visit(root, root, &mut result)?;
    }
    Ok(result)
}

fn no_staging(root: &Path) -> Result<()> {
    for entry in std::fs::read_dir(root)? {
        assert!(
            !entry?
                .file_name()
                .to_string_lossy()
                .starts_with(".staging-")
        );
    }
    Ok(())
}

#[tokio::test]
async fn interrupted_and_tampered_imports_keep_previous_release_unchanged() -> Result<()> {
    let fixture = Fixture::new()?;
    import(
        &fixture.state,
        bundle("0.3.0", (1, 1), b"first agent", None)?,
    )
    .await?;
    let before = snapshot(&fixture.release_root())?;
    // 0.4.0+ releases are strictly self-contained raw Agent binaries and reject separate archives.
    assert!(matches!(
        import(
            &fixture.state,
            bundle("0.4.0", (1, 1), b"next agent", Some(b"legacy runtime"))?,
        )
        .await,
        Err(ApiError::Conflict(_))
    ));
    assert_eq!(snapshot(&fixture.release_root())?, before);

    let candidate = bundle("0.4.0", (1, 1), b"next agent", None)?;
    let assets = candidate.assets.clone();
    let result = releases::import_bundle_for_targets(
        &fixture.state,
        &candidate.tag,
        candidate.proof.clone(),
        &[native_arch()?.into()],
        move |name, _| {
            let bytes = assets.get(&name).cloned();
            async move {
                if name.starts_with("agent-") {
                    bail!("fixture download interrupted during Agent staging");
                }
                bytes.context("fixture asset missing")
            }
        },
    )
    .await;
    assert!(matches!(result, Err(ApiError::Conflict(_))));
    assert_eq!(snapshot(&fixture.release_root())?, before);
    no_staging(&fixture.release_root())?;
    let mut tampered = candidate.clone();
    tampered
        .assets
        .values_mut()
        .find(|bytes| bytes.as_slice() == b"next agent")
        .unwrap()[0] ^= 1;
    assert!(matches!(
        import(&fixture.state, tampered).await,
        Err(ApiError::Conflict(_))
    ));
    assert_eq!(snapshot(&fixture.release_root())?, before);
    no_staging(&fixture.release_root())?;
    let mut unsigned = candidate;
    unsigned.proof.signature.clear();
    assert!(matches!(
        import(&fixture.state, unsigned).await,
        Err(ApiError::Conflict(_))
    ));
    assert_eq!(snapshot(&fixture.release_root())?, before);
    assert_eq!(
        releases::select_agent(&fixture.state, None).await?,
        ("0.3.0".into(), "agent-v0.3.0".into())
    );
    Ok(())
}

#[tokio::test]
async fn idempotence_reuses_valid_bytes_and_repairs_damaged_signed_payloads() -> Result<()> {
    let fixture = Fixture::new()?;
    let bundle = bundle("0.3.0", (1, 1), b"agent", Some(b"runtime"))?;
    assert_eq!(import(&fixture.state, bundle.clone()).await?, 1);
    let before = snapshot(&fixture.release_root())?;
    let count = releases::import_bundle_for_targets(
        &fixture.state,
        &bundle.tag,
        bundle.proof.clone(),
        &[native_arch()?.into()],
        |_, _| async { bail!("idempotent import must use verified existing bytes") },
    )
    .await?;
    assert_eq!(count, 1);
    assert_eq!(snapshot(&fixture.release_root())?, before);
    let agents = releases::signed_agents(&fixture.state).await?;
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].sha256, signing::hash(b"agent"));
    let path = fixture
        .release_root()
        .join(&bundle.tag)
        .join("agent/0.3.0")
        .join(native_arch()?);
    std::fs::write(&path, b"changed bytes")?;
    assert_eq!(import(&fixture.state, bundle).await?, 1);
    assert_eq!(std::fs::read(&path)?, b"agent");
    let repaired = releases::signed_agents(&fixture.state).await?;
    assert_eq!(repaired[0].sha256, signing::hash(b"agent"));
    Ok(())
}

#[tokio::test]
async fn immutable_agent_hash_conflicts_reject_new_tags() -> Result<()> {
    let fixture = Fixture::new()?;
    let original = bundle("0.3.0", (1, 1), b"agent first", None)?;
    import(&fixture.state, original).await?;
    let before = snapshot(&fixture.release_root())?;

    let mut conflicting = bundle("0.3.0", (1, 1), b"agent different", None)?;
    let mut metadata: ReleaseMetadata = serde_json::from_str(&conflicting.proof.metadata_json)?;
    metadata.tag = "agent-v0.3.1".into();
    conflicting.tag = metadata.tag.clone();
    conflicting.proof.metadata_json = format!("{}\n", serde_json::to_string(&metadata)?);
    conflicting.proof.checksums = conflicting
        .proof
        .checksums
        .lines()
        .map(|line| {
            if line.ends_with("  release.json") {
                format!(
                    "{}  release.json\n",
                    signing::hash(conflicting.proof.metadata_json.as_bytes())
                )
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    conflicting.proof.signature = signing::sign(conflicting.proof.checksums.as_bytes());

    assert!(matches!(
        import(&fixture.state, conflicting).await,
        Err(ApiError::Conflict(_))
    ));
    assert_eq!(snapshot(&fixture.release_root())?, before);
    Ok(())
}

#[tokio::test]
async fn latest_agent_is_selected_numerically_with_signed_protocol_compatibility() -> Result<()> {
    let fixture = Fixture::new()?;
    for (version, protocol) in [
        ("0.9.0", (1, 1)),
        ("0.10.0", (1, 1)),
        ("9.0.0", (2, 2)),
        ("0.11.0-rc.1", (1, 1)),
    ] {
        import(
            &fixture.state,
            bundle(version, protocol, version.as_bytes(), None)?,
        )
        .await?;
    }
    assert_eq!(
        releases::select_agent(&fixture.state, None).await?,
        ("0.10.0".into(), "agent-v0.10.0".into())
    );
    assert_eq!(
        releases::select_agent(&fixture.state, Some("0.9.0")).await?,
        ("0.9.0".into(), "agent-v0.9.0".into())
    );
    assert!(matches!(
        releases::select_agent(&fixture.state, Some("9.0.0")).await,
        Err(ApiError::Conflict(_))
    ));
    assert_eq!(
        releases::select_agent(&fixture.state, Some("0.11.0-rc.1")).await?,
        ("0.11.0-rc.1".into(), "agent-v0.11.0-rc.1".into())
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_ancestors_and_component_paths_are_rejected_without_external_writes() -> Result<()>
{
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new()?;
    let outside = fixture.root.join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("sentinel"), b"preserve external files")?;
    symlink(&outside, fixture.state.config.data_dir.join("artifacts"))?;
    let candidate = bundle("0.3.0", (1, 1), b"agent", None)?;
    assert!(matches!(
        import(&fixture.state, candidate.clone()).await,
        Err(ApiError::Conflict(_))
    ));
    assert_eq!(
        snapshot(&outside)?,
        BTreeMap::from([(
            PathBuf::from("sentinel"),
            b"preserve external files".to_vec()
        )])
    );
    assert!(matches!(
        releases::signed_agents(&fixture.state).await,
        Err(ApiError::Conflict(_))
    ));
    std::fs::remove_file(fixture.state.config.data_dir.join("artifacts"))?;
    import(&fixture.state, candidate.clone()).await?;
    let component = fixture
        .release_root()
        .join(&candidate.tag)
        .join("agent/0.3.0");
    let moved = fixture.root.join("moved-component");
    std::fs::rename(&component, &moved)?;
    symlink(&moved, &component)?;
    let before = snapshot(&moved)?;
    assert!(matches!(
        releases::signed_agents(&fixture.state).await,
        Err(ApiError::Conflict(_))
    ));
    assert!(matches!(
        import(&fixture.state, candidate).await,
        Err(ApiError::Conflict(_))
    ));
    assert_eq!(snapshot(&moved)?, before);
    Ok(())
}

#[tokio::test]
async fn missing_trust_root_and_custom_source_fields_are_rejected() -> Result<()> {
    let mut fixture = Fixture::new()?;
    fixture.state.release_keys = None;
    assert!(matches!(
        import(&fixture.state, bundle("0.3.0", (1, 1), b"agent", None)?).await,
        Err(ApiError::Conflict(_))
    ));
    assert!(!fixture.release_root().exists());
    assert!(matches!(
        releases::signed_agents(&fixture.state).await,
        Err(ApiError::Conflict(_))
    ));
    assert!(matches!(
        releases::select_agent(&fixture.state, None).await,
        Err(ApiError::Conflict(_))
    ));
    assert!(
        serde_json::from_value::<releases::ImportRequest>(serde_json::json!({
            "tag":"agent-v0.3.0", "url":"https://untrusted.example.test/"
        }))
        .is_err()
    );
    Ok(())
}

#[test]
fn verify_payload_accepts_only_raw_agent_entries() -> Result<()> {
    let binary = b"runtime fixture";
    let archive = release_fixture::archive("sing-box", binary)?;
    let plugin_entry = signing::entry(
        "sing-box",
        "1.14.4-apsp.2",
        "sing-box",
        "tar.gz",
        &archive,
        binary,
    );
    let agent_bytes = b"raw agent binary";
    let agent_entry = signing::entry(
        "agent",
        "0.3.0",
        "apsp-agent",
        "raw",
        agent_bytes,
        agent_bytes,
    );
    let proof = signing::signed_release(vec![
        (agent_entry, agent_bytes.to_vec()),
        (plugin_entry, archive.clone()),
    ]);
    let release = apsp_protocol::release::verify_release(&proof, &signing::trusted_keys())?;
    let plugin_artifact = release.artifact("sing-box", "1.14.4-apsp.2", native_arch()?)?;
    assert!(releases::verify_payload(&plugin_artifact, &archive).is_err());
    let agent_artifact = release.artifact("agent", "0.3.0", native_arch()?)?;
    releases::verify_payload(&agent_artifact, agent_bytes)?;
    Ok(())
}

#[tokio::test]
async fn verify_running_component_binds_structured_manifest_cross_abi_sha_and_rejects_spoofing()
-> Result<()> {
    use apsp_protocol::release::canonical_asset_name;
    use release_fixture::{build_signed_agent_entry, pinned_components, write_entries_tagged};
    use serde_json::json;

    let fixture = Fixture::new()?;
    let components = pinned_components(&["sing-box", "ipquality", "tcpquality"])?;
    let ipquality_version = apsp_observability::diagnostic_tools::ipquality::TOOL_VERSION;
    let tcp_version = apsp_observability::diagnostic_tools::tcpquality::TOOL_VERSION;
    let stray_rodata_hex = "e".repeat(64);

    let mut artifacts = Vec::new();
    let mut musl_agent = None;
    for target in ["linux-gnu-amd64", "linux-musl-amd64"] {
        let (_, mut agent) = build_signed_agent_entry("0.4.0", target, components.clone())?;
        agent
            .raw_binary
            .extend_from_slice(stray_rodata_hex.as_bytes());
        let mut entry = signing::entry(
            "agent",
            "0.4.0",
            "apsp-agent",
            "raw",
            &agent.raw_binary,
            &agent.raw_binary,
        );
        entry.arch = target.into();
        entry.asset_name = canonical_asset_name(&entry)?;
        let agent_sha = entry.binary_sha256.clone();
        agent.agent_bytes = entry.archive_size;
        agent
            .component_digests
            .insert("agent".into(), agent_sha.clone());
        agent
            .component_digests
            .insert("tcpquality".into(), agent_sha.clone());
        agent.agent_sha256 = agent_sha;
        artifacts.push((entry, agent.raw_binary.clone()));
        if target == "linux-musl-amd64" {
            musl_agent = Some(agent);
        }
    }
    write_entries_tagged(&fixture.state.config.data_dir, "agent-v0.4.0", artifacts)?;
    let musl = musl_agent.context("linux-musl-amd64 signed test Agent")?;
    let musl_sha = musl.agent_sha256.clone();
    let singbox_sha = musl.component_digests["sing-box"].clone();
    let ipquality_sha = musl.component_digests["ipquality"].clone();

    let now = apsp_protocol::now_timestamp();
    // Host reports libc="gnu", so linux-gnu-amd64 is the first target candidate,
    // while the running static Agent binary is linux-musl-amd64 (musl_sha).
    let mut valid_info = musl.merge_static_info(&json!({}));
    valid_info["libc"] = json!("gnu");

    let ipq = releases::verify_running_component(
        &fixture.state,
        &valid_info,
        Some(now),
        Some(now),
        now,
        "ipquality",
        ipquality_version,
    )
    .await?;
    assert_eq!(ipq.target, "linux-musl-amd64");
    assert_eq!(ipq.agent_sha256, musl_sha);
    assert_eq!(ipq.component_sha256, ipquality_sha);

    let tcp = releases::verify_running_component(
        &fixture.state,
        &valid_info,
        Some(now),
        Some(now),
        now,
        "tcpquality",
        tcp_version,
    )
    .await?;
    assert_eq!(tcp.component_sha256, musl_sha);

    // Cross-component spoofing: claiming sing-box's signed digest for ipquality must fail!
    let mut swapped = valid_info.clone();
    swapped["component_digests"]["ipquality"] = json!(singbox_sha);
    assert!(matches!(
        releases::verify_running_component(
            &fixture.state,
            &swapped,
            Some(now),
            Some(now),
            now,
            "ipquality",
            ipquality_version,
        )
        .await,
        Err(ApiError::Conflict(_))
    ));

    // Unrelated 64-char hex present in .rodata must never validate as a component digest.
    let mut rodata_spoof = valid_info.clone();
    rodata_spoof["component_digests"]["ipquality"] = json!(stray_rodata_hex);
    assert!(matches!(
        releases::verify_running_component(
            &fixture.state,
            &rodata_spoof,
            Some(now),
            Some(now),
            now,
            "ipquality",
            ipquality_version,
        )
        .await,
        Err(ApiError::Conflict(_))
    ));

    // Legacy 0.3.0 Agent without embedded manifest frame must never pass verify_running_component.
    let legacy_bytes = format!("legacy-agent-{stray_rodata_hex}").into_bytes();
    let legacy_sha = signing::hash(&legacy_bytes);
    import(
        &fixture.state,
        bundle("0.3.0", (1, 1), &legacy_bytes, None)?,
    )
    .await?;
    let legacy_info = json!({
        "os": "linux",
        "arch": native_arch()?,
        "agent_version": "0.3.0",
        "components": { "ipquality": ipquality_version },
        "component_digests": {
            "agent": legacy_sha,
            "ipquality": stray_rodata_hex,
        }
    });
    assert!(matches!(
        releases::verify_running_component(
            &fixture.state,
            &legacy_info,
            Some(now),
            Some(now),
            now,
            "ipquality",
            ipquality_version,
        )
        .await,
        Err(ApiError::Conflict(_))
    ));
    Ok(())
}

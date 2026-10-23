#![forbid(unsafe_code)]

mod release_fixture;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
#[path = "releases/targets.rs"]
mod targets;
#[path = "releases/versions.rs"]
mod versions;

use anyhow::{Context, Result, bail};
use apsp_panel::{AppState, config::Config, error::ApiError, releases};
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
            device_lifecycle: Default::default(),
            telemetry_live: Arc::default(),
            pool: PgPoolOptions::new().connect_lazy("postgres://fixture@127.0.0.1/unused")?,
            login_permits: Arc::new(Semaphore::new(4)),
            passkeys: Arc::new(apsp_panel::passkeys::Service::new("http://127.0.0.1")),
            quality_permits: Arc::new(Semaphore::new(2)),
            quality_providers: Arc::default(),
            release_permits: Arc::new(Semaphore::new(1)),
            release_keys: Some(Arc::new(signing::trusted_keys())),
            config: Arc::new(Config {
                database_url: "postgres://fixture@127.0.0.1/unused".into(),
                listen: "127.0.0.1:0".parse()?,
                public_url: "http://127.0.0.1".into(),
                data_dir: data,
                admin_password: None,
            }),
            connections: Arc::default(),
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
            signing::entry("sing-box", "1.14.2", "sing-box", "tar.gz", &archive, binary),
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
    let candidate = bundle("0.4.0", (1, 1), b"next agent", Some(b"runtime"))?;
    let assets = candidate.assets.clone();
    let result = releases::import_bundle_for_targets(
        &fixture.state,
        &candidate.tag,
        candidate.proof.clone(),
        &[native_arch()?.into()],
        move |name, _| {
            let bytes = assets.get(&name).cloned();
            async move {
                if name.starts_with("sing-box-") {
                    bail!("fixture download interrupted after Agent staging");
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
    assert_eq!(import(&fixture.state, bundle.clone()).await?, 2);
    let before = snapshot(&fixture.release_root())?;
    let count = releases::import_bundle_for_targets(
        &fixture.state,
        &bundle.tag,
        bundle.proof.clone(),
        &[native_arch()?.into()],
        |_, _| async { bail!("idempotent import must use verified existing bytes") },
    )
    .await?;
    assert_eq!(count, 2);
    assert_eq!(snapshot(&fixture.release_root())?, before);
    assert_eq!(releases::entries(&fixture.state).await?.len(), 2);
    let path = fixture
        .release_root()
        .join(&bundle.tag)
        .join("agent/0.3.0")
        .join(native_arch()?);
    std::fs::write(&path, b"changed bytes")?;
    assert_eq!(import(&fixture.state, bundle).await?, 2);
    assert_eq!(std::fs::read(&path)?, b"agent");
    assert_eq!(
        releases::artifact(&fixture.state, "agent", "0.3.0", native_arch()?)
            .await?
            .0,
        b"agent"
    );
    Ok(())
}

#[tokio::test]
async fn immutable_component_hash_conflicts_reject_new_tags() -> Result<()> {
    let fixture = Fixture::new()?;
    import(
        &fixture.state,
        bundle("0.3.0", (1, 1), b"agent first", Some(b"runtime first"))?,
    )
    .await?;
    let before = snapshot(&fixture.release_root())?;
    let conflicting = bundle("0.4.0", (1, 1), b"agent next", Some(b"runtime different"))?;
    assert!(matches!(
        import(&fixture.state, conflicting).await,
        Err(ApiError::Conflict(_))
    ));
    assert_eq!(snapshot(&fixture.release_root())?, before);
    import(
        &fixture.state,
        bundle("0.4.0", (1, 1), b"agent next", Some(b"runtime first"))?,
    )
    .await?;
    assert_eq!(releases::entries(&fixture.state).await?.len(), 3);
    let (runtime, _, _) =
        releases::artifact(&fixture.state, "sing-box", "1.14.2", native_arch()?).await?;
    assert!(!runtime.is_empty());
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
        releases::entries(&fixture.state).await,
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
        releases::artifact(&fixture.state, "agent", "0.3.0", native_arch()?).await,
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
        releases::entries(&fixture.state).await,
        Err(ApiError::Conflict(_))
    ));
    assert!(matches!(
        releases::artifact(&fixture.state, "agent", "0.3.0", native_arch()?).await,
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
fn native_runtime_archives_require_the_exact_signed_auxiliary_files() -> Result<()> {
    let binary = b"runtime-fixture";
    let dll = b"dll-fixture";
    for (files, accepted) in [
        (
            vec![
                ("sing-box.exe", binary.as_slice()),
                ("wintun.dll", dll.as_slice()),
            ],
            true,
        ),
        (vec![("sing-box.exe", binary.as_slice())], false),
        (
            vec![
                ("sing-box.exe", binary.as_slice()),
                ("wintun.dll", b"changed".as_slice()),
            ],
            false,
        ),
        (
            vec![
                ("sing-box.exe", binary.as_slice()),
                ("wintun.dll", dll.as_slice()),
                ("extra.dll", dll.as_slice()),
            ],
            false,
        ),
        (
            vec![
                ("sing-box.exe", binary.as_slice()),
                ("wintun.dll", dll.as_slice()),
                ("wintun.dll", dll.as_slice()),
            ],
            false,
        ),
    ] {
        let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        for (name, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            archive.append_data(&mut header, name, bytes)?;
        }
        let bytes = archive.into_inner()?.finish()?;
        let mut entry = signing::entry(
            "sing-box",
            "1.14.2",
            "sing-box.exe",
            "tar.gz",
            &bytes,
            binary,
        );
        entry.arch = "windows-amd64".into();
        entry.asset_name = apsp_protocol::release::canonical_asset_name(&entry)?;
        entry.auxiliary_files.insert(
            "wintun.dll".into(),
            apsp_protocol::release::ReleaseFile {
                sha256: signing::hash(dll),
                size: dll.len() as u64,
            },
        );
        let proof = signing::signed_release(vec![(entry, bytes.clone())]);
        let release = apsp_protocol::release::verify_release(&proof, &signing::trusted_keys())?;
        let artifact = release.artifact("sing-box", "1.14.2", "windows-amd64")?;
        assert_eq!(
            releases::verify_payload(&artifact, &bytes).is_ok(),
            accepted
        );
    }
    Ok(())
}

#[test]
fn concatenated_gzip_cannot_hide_nonzero_archive_tail() -> Result<()> {
    use std::io::Write;
    let binary = b"runtime fixture";
    let mut bytes = release_fixture::archive("sing-box", binary)?;
    let mut tail = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    tail.write_all(b"unlisted trailing payload")?;
    bytes.extend(tail.finish()?);
    let entry = signing::entry("sing-box", "1.14.2", "sing-box", "tar.gz", &bytes, binary);
    let proof = signing::signed_release(vec![(entry, bytes.clone())]);
    let release = apsp_protocol::release::verify_release(&proof, &signing::trusted_keys())?;
    let artifact = release.artifact("sing-box", "1.14.2", native_arch()?)?;
    assert!(releases::verify_payload(&artifact, &bytes).is_err());
    Ok(())
}

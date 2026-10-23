use super::*;
use apsp_protocol::release::canonical_asset_name;
use std::sync::Mutex;

pub(super) fn multi_arch_bundle() -> Result<Bundle> {
    let mut artifacts = Vec::new();
    for (arch, agent, runtime) in [
        ("arm64", b"arm agent".as_slice(), b"arm runtime".as_slice()),
        ("amd64", b"amd agent".as_slice(), b"amd runtime".as_slice()),
    ] {
        let mut entry = signing::entry("agent", "0.3.0", "apsp-agent", "raw", agent, agent);
        entry.arch = arch.into();
        entry.asset_name = canonical_asset_name(&entry)?;
        artifacts.push((entry, agent.to_vec()));
        let archive = release_fixture::archive("sing-box", runtime)?;
        let mut entry = signing::entry(
            "sing-box", "1.14.2", "sing-box", "tar.gz", &archive, runtime,
        );
        entry.arch = arch.into();
        entry.asset_name = canonical_asset_name(&entry)?;
        artifacts.push((entry, archive));
    }
    let proof = signing::signed_release(artifacts.clone());
    Ok(Bundle {
        tag: "agent-v0.3.0".into(),
        proof,
        assets: artifacts
            .into_iter()
            .map(|(entry, bytes)| (entry.asset_name, bytes))
            .chain(std::iter::once((
                "install.sh".into(),
                b"#!/bin/sh\nexit 0\n".to_vec(),
            )))
            .collect(),
    })
}

pub(super) async fn import_target(
    state: &AppState,
    bundle: &Bundle,
    target: &str,
    downloads: &Mutex<Vec<String>>,
) -> Result<usize, ApiError> {
    releases::import_bundle_for_targets(
        state,
        &bundle.tag,
        bundle.proof.clone(),
        &[target.into()],
        |name, maximum| {
            downloads.lock().unwrap().push(name.clone());
            let bytes = bundle
                .assets
                .get(&name)
                .cloned()
                .context("fixture asset missing");
            async move {
                let bytes = bytes?;
                anyhow::ensure!(bytes.len() <= maximum, "fixture exceeds bound");
                Ok(bytes)
            }
        },
    )
    .await
}

#[tokio::test]
async fn arm_import_downloads_only_arm_and_can_append_amd_without_redownloading() -> Result<()> {
    let fixture = Fixture::new()?;
    let bundle = multi_arch_bundle()?;
    let downloads = Mutex::new(Vec::new());
    assert_eq!(
        import_target(&fixture.state, &bundle, "linux-gnu-arm64", &downloads).await?,
        2
    );
    assert!(
        downloads
            .lock()
            .unwrap()
            .iter()
            .all(|name| !name.contains("amd64"))
    );
    assert_eq!(downloads.lock().unwrap().len(), 3);
    let entries = releases::entries(&fixture.state).await?;
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|entry| entry.arch == "arm64"));
    assert!(matches!(
        releases::artifact(&fixture.state, "agent", "0.3.0", "amd64").await,
        Err(ApiError::NotFound)
    ));
    // Agent updates use the complete signed proof even when another ABI is cached.
    let update = releases::newer_agent(&fixture.state, &["amd64".into()], (0, 2, 0))
        .await?
        .context("signed GitHub update missing")?;
    assert_eq!(update.version, "0.3.0");
    assert_eq!(
        update.artifact.url,
        "https://github.com/theLucius7/apsp/releases/download/agent-v0.3.0/agent-0.3.0-linux-musl-amd64"
    );
    assert_eq!(update.artifact.sha256, signing::hash(b"amd agent"));
    assert_eq!(update.artifact.proof, Some(bundle.proof.clone()));
    assert_eq!(downloads.lock().unwrap().len(), 3);
    assert!(
        releases::newer_agent(&fixture.state, &["arm64".into()], (0, 2, 0))
            .await?
            .is_some()
    );
    let proof = releases::proof_at(&fixture.release_root().join(&bundle.tag)).await?;
    let metadata: ReleaseMetadata = serde_json::from_str(&proof.metadata_json)?;
    assert_eq!(metadata.artifacts.len(), 4);
    assert_eq!(proof, bundle.proof);
    downloads.lock().unwrap().clear();
    assert_eq!(
        import_target(&fixture.state, &bundle, "linux-gnu-amd64", &downloads).await?,
        4
    );
    assert_eq!(downloads.lock().unwrap().len(), 2);
    assert!(
        downloads
            .lock()
            .unwrap()
            .iter()
            .all(|name| name.contains("amd64"))
    );
    assert_eq!(releases::entries(&fixture.state).await?.len(), 4);
    downloads.lock().unwrap().clear();
    assert_eq!(
        import_target(&fixture.state, &bundle, "arm64", &downloads).await?,
        4
    );
    assert!(downloads.lock().unwrap().is_empty());
    no_staging(&fixture.release_root())?;
    Ok(())
}

#[tokio::test]
async fn interrupted_append_keeps_inventory_and_existing_artifacts_unchanged() -> Result<()> {
    let fixture = Fixture::new()?;
    let bundle = multi_arch_bundle()?;
    import_target(&fixture.state, &bundle, "arm64", &Mutex::new(Vec::new())).await?;
    let before = snapshot(&fixture.release_root())?;
    let result = releases::import_bundle_for_targets(
        &fixture.state,
        &bundle.tag,
        bundle.proof.clone(),
        &["amd64".into()],
        |name, _| {
            let bytes = bundle.assets.get(&name).cloned();
            async move {
                if name.starts_with("sing-box-") {
                    bail!("fixture interrupted while appending runtime");
                }
                bytes.context("fixture asset missing")
            }
        },
    )
    .await;
    assert!(matches!(result, Err(ApiError::Conflict(_))));
    assert_eq!(snapshot(&fixture.release_root())?, before);
    no_staging(&fixture.release_root())?;
    Ok(())
}

#[tokio::test]
async fn inventory_rejects_unsigned_paths_duplicates_and_missing_advertised_payloads() -> Result<()>
{
    let fixture = Fixture::new()?;
    let bundle = multi_arch_bundle()?;
    import_target(&fixture.state, &bundle, "arm64", &Mutex::new(Vec::new())).await?;
    let directory = fixture.release_root().join(&bundle.tag);
    let inventory = directory.join("inventory.json");
    let original = std::fs::read(&inventory)?;
    for paths in [
        vec!["agent/0.3.0/arm64", "agent/0.3.0/arm64"],
        vec!["agent/9.0.0/arm64"],
        vec!["../../outside"],
        vec![],
    ] {
        std::fs::write(
            &inventory,
            serde_json::to_vec(&serde_json::json!({"paths": paths}))?,
        )?;
        assert!(matches!(
            releases::entries(&fixture.state).await,
            Err(ApiError::Conflict(_))
        ));
        assert!(matches!(
            import_target(&fixture.state, &bundle, "arm64", &Mutex::new(Vec::new())).await,
            Err(ApiError::Conflict(_))
        ));
    }
    std::fs::write(&inventory, original)?;
    std::fs::remove_file(directory.join("agent/0.3.0/arm64"))?;
    assert!(matches!(
        releases::entries(&fixture.state).await,
        Err(ApiError::Conflict(_))
    ));
    // The signed catalogue can still issue a command that repairs this Agent on demand.
    assert_eq!(
        releases::select_agent(&fixture.state, None).await?,
        ("0.3.0".into(), "agent-v0.3.0".into())
    );
    let downloads = Mutex::new(Vec::new());
    assert_eq!(
        import_target(&fixture.state, &bundle, "arm64", &downloads).await?,
        2
    );
    assert_eq!(downloads.lock().unwrap().len(), 1);
    assert!(downloads.lock().unwrap()[0].contains("arm64"));
    assert_eq!(releases::entries(&fixture.state).await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn legacy_complete_imports_remain_usable_and_corrupt_unrequested_abis_are_not_downloaded()
-> Result<()> {
    let fixture = Fixture::new()?;
    let bundle = multi_arch_bundle()?;
    let downloads = Mutex::new(Vec::new());
    import_target(&fixture.state, &bundle, "arm64", &downloads).await?;
    import_target(&fixture.state, &bundle, "amd64", &downloads).await?;
    let directory = fixture.release_root().join(&bundle.tag);
    std::fs::remove_file(directory.join("inventory.json"))?;
    assert_eq!(releases::entries(&fixture.state).await?.len(), 4);
    downloads.lock().unwrap().clear();
    assert_eq!(
        import_target(&fixture.state, &bundle, "arm64", &downloads).await?,
        4
    );
    assert!(downloads.lock().unwrap().is_empty());
    let stale = directory.join("agent/0.3.0/amd64");
    std::fs::write(&stale, b"damaged AMD payload")?;
    assert_eq!(
        import_target(&fixture.state, &bundle, "arm64", &downloads).await?,
        3
    );
    assert!(downloads.lock().unwrap().is_empty());
    assert_eq!(std::fs::read(&stale)?, b"damaged AMD payload");
    assert_eq!(releases::entries(&fixture.state).await?.len(), 3);
    Ok(())
}

#[tokio::test]
async fn target_validation_and_unavailable_abi_fail_before_any_asset_download() -> Result<()> {
    let fixture = Fixture::new()?;
    let bundle = multi_arch_bundle()?;
    for targets in [
        vec![],
        vec!["arm64".into(), "arm64".into()],
        vec!["riscv64".into()],
        vec!["windows-arm64".into()],
    ] {
        assert!(matches!(
            releases::import_bundle_for_targets(
                &fixture.state,
                &bundle.tag,
                bundle.proof.clone(),
                &targets,
                |_, _| async { bail!("invalid target must not fetch any payload") },
            )
            .await,
            Err(ApiError::BadRequest(_))
        ));
    }
    assert!(!fixture.release_root().exists());
    Ok(())
}

#[tokio::test]
async fn partial_imports_still_reserve_unavailable_signed_artifact_identities() -> Result<()> {
    let fixture = Fixture::new()?;
    let original = multi_arch_bundle()?;
    import_target(&fixture.state, &original, "arm64", &Mutex::new(Vec::new())).await?;
    let before = snapshot(&fixture.release_root())?;
    let mut conflicting = original.clone();
    let mut metadata: ReleaseMetadata = serde_json::from_str(&conflicting.proof.metadata_json)?;
    metadata.tag = "agent-v0.4.0".into();
    let entry = metadata
        .artifacts
        .iter_mut()
        .find(|entry| entry.name == "agent" && entry.arch == "amd64")
        .context("fixture AMD Agent missing")?;
    let path = format!("{}/{}/{}", entry.name, entry.version, entry.arch);
    let bytes = b"other AMD Agent";
    entry.binary_sha256 = signing::hash(bytes);
    entry.archive_size = bytes.len() as u64;
    entry.binary_size = bytes.len() as u64;
    conflicting
        .assets
        .insert(entry.asset_name.clone(), bytes.to_vec());
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
            } else if line.ends_with(&format!("  {path}")) {
                format!("{}  {path}\n", signing::hash(bytes))
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    conflicting.proof.signature = signing::sign(conflicting.proof.checksums.as_bytes());
    assert!(matches!(
        import_target(
            &fixture.state,
            &conflicting,
            "arm64",
            &Mutex::new(Vec::new())
        )
        .await,
        Err(ApiError::Conflict(_))
    ));
    assert_eq!(snapshot(&fixture.release_root())?, before);
    Ok(())
}

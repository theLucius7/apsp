use super::*;
use apsp_protocol::release::canonical_asset_name;
use std::sync::Mutex;

fn platform_bundle(version: &str, protocol: (u16, u16), targets: &[&str]) -> Result<Bundle> {
    let artifacts: Vec<_> = targets
        .iter()
        .map(|target| {
            let binary = format!("fixture Agent {version} {target}").into_bytes();
            let name = if target.starts_with("windows-") {
                "apsp-agent.exe"
            } else {
                "apsp-agent"
            };
            let mut entry = signing::entry("agent", version, name, "raw", &binary, &binary);
            entry.arch = (*target).into();
            entry.asset_name = canonical_asset_name(&entry)?;
            Ok((entry, binary))
        })
        .collect::<Result<_>>()?;
    let mut proof = signing::signed_release(artifacts.clone());
    let mut metadata: ReleaseMetadata = serde_json::from_str(&proof.metadata_json)?;
    metadata.tag = format!("agent-v{version}");
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
    Ok(Bundle {
        tag: metadata.tag,
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

#[tokio::test]
async fn explicit_installation_distinguishes_missing_protocol_and_platform_failures() -> Result<()>
{
    let fixture = Fixture::new()?;
    for (version, protocol) in [("0.9.0", (1, 1)), ("9.0.0", (2, 2))] {
        let bundle = platform_bundle(version, protocol, &["linux-musl-amd64"])?;
        targets::import_target(
            &fixture.state,
            &bundle,
            "linux-musl-amd64",
            &Mutex::new(Vec::new()),
        )
        .await?;
    }
    for (version, target, expected) in [
        ("0.1.0", "linux-musl-amd64", "尚未导入"),
        ("9.0.0", "linux-musl-amd64", "协议不兼容"),
        ("0.9.0", "linux-musl-arm64", "该平台可安装的制品"),
    ] {
        let selected =
            releases::select_agent_for_target(&fixture.state, Some(version), Some(target)).await;
        match selected {
            Err(ApiError::Conflict(message)) => assert!(message.contains(expected), "{message}"),
            other => panic!("expected explicit selection failure, got {other:?}"),
        }
        let installation = apsp_panel::installation::select_with_mirror(
            &fixture.state,
            Some(version),
            "fixture-token",
            Some("unix"),
            Some(target),
            "",
        )
        .await;
        match installation {
            Err(ApiError::Conflict(message)) => assert!(message.contains(expected), "{message}"),
            _ => panic!("expected explicit installation failure"),
        }
    }
    let installed = apsp_panel::installation::select_with_mirror(
        &fixture.state,
        Some("0.9.0"),
        "fixture-token",
        Some("unix"),
        Some("linux-musl-amd64"),
        "",
    )
    .await?;
    assert_eq!(installed.version, "0.9.0");
    assert_eq!(installed.tag.as_deref(), Some("agent-v0.9.0"));
    Ok(())
}

#[tokio::test]
async fn signed_legacy_identity_is_preserved_but_not_offered_as_standard_installation() -> Result<()>
{
    let fixture = Fixture::new()?;
    let downloads = Mutex::new(Vec::new());
    for version in ["0.1.0", "0.2.0", "0.4.0"] {
        let bundle = platform_bundle(version, (1, 1), &["linux-musl-amd64"])?;
        targets::import_target(&fixture.state, &bundle, "linux-musl-amd64", &downloads).await?;
    }
    for version in ["0.1.0", "0.2.0"] {
        let result = releases::select_agent_for_target(
            &fixture.state,
            Some(version),
            Some("linux-musl-amd64"),
        )
        .await;
        match result {
            Err(ApiError::Conflict(message)) => {
                assert!(message.contains("历史 Agent 不支持当前标准安装与服务合同"));
                assert!(!message.contains("尚未导入"));
                assert!(!message.contains("协议不兼容"));
            }
            other => panic!("unsupported legacy installation was offered: {other:?}"),
        }
        assert!(
            releases::agent_versions(&fixture.state, None, Some(version))
                .await?
                .is_empty()
        );
        // The verified original release inventory remains readable; filtering
        // installation choices never deletes or rewrites a historical payload.
        let (bytes, _, _) =
            releases::artifact(&fixture.state, "agent", version, "linux-musl-amd64").await?;
        assert_eq!(
            bytes,
            format!("fixture Agent {version} linux-musl-amd64").as_bytes()
        );
    }
    let selected = apsp_panel::installation::select_with_mirror(
        &fixture.state,
        Some("0.4.0"),
        "TEST_ONLY fixture token",
        Some("unix"),
        Some("linux-musl-amd64"),
        "",
    )
    .await?;
    assert_eq!(selected.version, "0.4.0");
    assert_ne!(selected.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(selected.tag.as_deref(), Some("agent-v0.4.0"));
    assert!(selected.install_command.contains("'0.4.0'"));
    Ok(())
}

#[tokio::test]
async fn versions_are_numeric_platform_compatible_and_include_signed_uncached_targets() -> Result<()>
{
    let fixture = Fixture::new()?;
    let downloads = Mutex::new(Vec::new());
    for (version, protocol, targets, imported) in [
        ("0.9.0", (1, 1), vec!["amd64", "arm64"], "arm64"),
        (
            "0.10.0",
            (1, 2),
            vec!["linux-musl-arm64"],
            "linux-musl-arm64",
        ),
        ("1.0.0", (1, 1), vec!["windows-amd64"], "windows-amd64"),
        ("2.0.0", (1, 1), vec!["macos-arm64"], "macos-arm64"),
        ("3.0.0", (1, 1), vec!["freebsd-amd64"], "freebsd-amd64"),
        ("9.0.0", (2, 2), vec!["arm64"], "arm64"),
        ("4.0.0-rc.1", (1, 1), vec!["arm64"], "arm64"),
    ] {
        let bundle = platform_bundle(version, protocol, &targets)?;
        targets::import_target(&fixture.state, &bundle, imported, &downloads).await?;
    }
    let versions = releases::agent_versions(&fixture.state, None, None).await?;
    assert_eq!(
        versions
            .iter()
            .map(|entry| entry.version.as_str())
            .collect::<Vec<_>>(),
        ["3.0.0", "2.0.0", "1.0.0", "0.10.0", "0.9.0"]
    );
    let first = versions.last().context("Linux fixture missing")?;
    assert_eq!(first.targets, ["amd64", "arm64"]);
    assert_eq!(first.cached_targets, ["arm64"]);
    assert_eq!(
        releases::select_agent(&fixture.state, None).await?,
        ("0.10.0".into(), "agent-v0.10.0".into())
    );
    let arm = releases::agent_versions(&fixture.state, Some("linux-gnu-arm64"), None).await?;
    assert_eq!(
        arm.iter()
            .map(|entry| entry.version.as_str())
            .collect::<Vec<_>>(),
        ["0.10.0", "0.9.0"]
    );
    assert_eq!(
        releases::select_agent_for_target(&fixture.state, None, Some("windows-amd64")).await?,
        ("1.0.0".into(), "agent-v1.0.0".into())
    );
    assert_eq!(
        releases::select_agent_for_target(&fixture.state, None, Some("freebsd-amd64")).await?,
        ("3.0.0".into(), "agent-v3.0.0".into())
    );
    assert!(matches!(
        releases::select_agent_for_target(&fixture.state, Some("0.10.0"), Some("windows-amd64"))
            .await,
        Err(ApiError::Conflict(_))
    ));
    assert!(matches!(
        releases::agent_versions(&fixture.state, Some("riscv64"), None).await,
        Err(ApiError::BadRequest(_))
    ));
    let prerelease =
        releases::agent_versions(&fixture.state, Some("linux-musl-arm64"), Some("4.0.0-rc.1"))
            .await?;
    assert_eq!(prerelease[0].version, "4.0.0-rc.1");
    assert_eq!(prerelease[0].tag, "agent-v4.0.0-rc.1");
    Ok(())
}

#[tokio::test]
async fn native_prereleases_are_excluded_while_linux_prereleases_remain_selectable() -> Result<()> {
    let fixture = Fixture::new()?;
    let version = "4.1.0-rc.1";
    let native_targets = ["macos-arm64", "windows-arm64", "freebsd-arm64"];
    let bundle = platform_bundle(
        version,
        (1, 1),
        &[
            "linux-musl-arm64",
            "macos-arm64",
            "windows-arm64",
            "freebsd-arm64",
        ],
    )?;
    targets::import_target(
        &fixture.state,
        &bundle,
        "linux-musl-arm64",
        &Mutex::new(Vec::new()),
    )
    .await?;
    let explicit = releases::agent_versions(&fixture.state, None, Some(version)).await?;
    assert_eq!(explicit.len(), 1);
    assert_eq!(explicit[0].version, version);
    assert_eq!(explicit[0].targets, ["linux-musl-arm64"]);
    for target in native_targets {
        assert!(
            releases::agent_versions(&fixture.state, Some(target), Some(version))
                .await?
                .is_empty()
        );
        assert!(matches!(
            releases::select_agent_for_target(&fixture.state, Some(version), Some(target)).await,
            Err(ApiError::Conflict(_))
        ));
    }
    assert!(
        releases::agent_versions(&fixture.state, Some("linux-musl-arm64"), None)
            .await?
            .is_empty()
    );
    assert_eq!(
        releases::select_agent_for_target(&fixture.state, Some(version), Some("linux-musl-arm64"))
            .await?,
        (version.into(), format!("agent-v{version}"))
    );
    Ok(())
}

#[tokio::test]
async fn github_updates_use_signed_identities_without_cached_target_payloads() -> Result<()> {
    let fixture = Fixture::new()?;
    let bundle = targets::multi_arch_bundle()?;
    let downloads = Mutex::new(Vec::new());
    targets::import_target(&fixture.state, &bundle, "arm64", &downloads).await?;
    let before = snapshot(&fixture.release_root())?;
    let downloaded = downloads.lock().unwrap().clone();
    let release = releases::newer_agent(&fixture.state, &["amd64".into()], (0, 2, 0))
        .await?
        .context("signed uncached update missing")?;
    assert_eq!(release.version, "0.3.0");
    assert_eq!(release.download_mirror, "");
    assert_eq!(
        release.artifact.url,
        format!(
            "https://github.com/theLucius7/apsp/releases/download/{}/agent-0.3.0-linux-musl-amd64",
            bundle.tag
        )
    );
    assert_eq!(release.artifact.proof, Some(bundle.proof));
    assert_eq!(snapshot(&fixture.release_root())?, before);
    assert_eq!(*downloads.lock().unwrap(), downloaded);
    Ok(())
}

#[tokio::test]
async fn corrupt_cached_agent_remains_a_signed_github_candidate() -> Result<()> {
    let fixture = Fixture::new()?;
    let bundle = targets::multi_arch_bundle()?;
    targets::import_target(&fixture.state, &bundle, "arm64", &Mutex::new(Vec::new())).await?;
    let path = fixture
        .release_root()
        .join(&bundle.tag)
        .join("agent/0.3.0/arm64");
    std::fs::write(&path, b"damaged")?;
    let catalogue =
        releases::agent_versions(&fixture.state, Some("linux-musl-arm64"), None).await?;
    assert_eq!(catalogue[0].targets, ["arm64"]);
    assert!(catalogue[0].cached_targets.is_empty());
    let release = releases::newer_agent(&fixture.state, &["arm64".into()], (0, 2, 0))
        .await?
        .context("signed update missing")?;
    assert!(
        release
            .artifact
            .url
            .ends_with("agent-0.3.0-linux-musl-arm64")
    );
    assert_eq!(std::fs::read(path)?, b"damaged");
    assert!(matches!(
        releases::entries(&fixture.state).await,
        Err(ApiError::Conflict(_))
    ));
    Ok(())
}

#[tokio::test]
async fn native_agents_remain_selectable_without_panel_payloads() -> Result<()> {
    let fixture = Fixture::new()?;
    let native_targets = ["windows-arm64", "macos-arm64", "freebsd-arm64"];
    let bundle = platform_bundle(
        "0.3.0",
        (1, 1),
        &[
            "linux-musl-arm64",
            "windows-arm64",
            "macos-arm64",
            "freebsd-arm64",
        ],
    )?;
    targets::import_target(
        &fixture.state,
        &bundle,
        "linux-musl-arm64",
        &Mutex::new(Vec::new()),
    )
    .await?;
    let before = snapshot(&fixture.release_root())?;
    for target in native_targets {
        let versions = releases::agent_versions(&fixture.state, Some(target), None).await?;
        assert_eq!(versions[0].targets, [target]);
        assert!(versions[0].cached_targets.is_empty());
        let release = releases::newer_agent(&fixture.state, &[target.into()], (0, 2, 0))
            .await?
            .context("native signed update missing")?;
        assert_eq!(release.version, "0.3.0");
        assert!(release.artifact.url.starts_with(
            "https://github.com/theLucius7/apsp/releases/download/agent-v0.3.0/agent-0.3.0-"
        ));
        assert!(release.artifact.url.ends_with(target));
        assert_eq!(release.artifact.proof, Some(bundle.proof.clone()));
        assert!(matches!(
            releases::artifact(&fixture.state, "agent", "0.3.0", target).await,
            Err(ApiError::NotFound)
        ));
    }
    assert_eq!(snapshot(&fixture.release_root())?, before);
    assert_eq!(releases::entries(&fixture.state).await?.len(), 1);
    Ok(())
}

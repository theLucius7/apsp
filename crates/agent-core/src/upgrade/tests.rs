use super::*;
use crate::{
    release_test_support::{entry, hash, signed_release, trusted_keys},
    system::SystemOps,
};
use apsp_protocol::Artifact;
use std::{os::unix::fs::PermissionsExt, path::PathBuf, sync::Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

struct Fixture {
    root: PathBuf,
    origin: String,
    release: Arc<Mutex<AgentRelease>>,
    bytes: Arc<Mutex<Vec<u8>>>,
    requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fixture {
    fn artifact(_origin: &str, version: &str, bytes: &[u8]) -> Artifact {
        let proof = signed_release(vec![(
            entry("agent", version, executable_name(), "raw", bytes, bytes),
            bytes.to_vec(),
        )]);
        let metadata: apsp_protocol::release::ReleaseMetadata =
            serde_json::from_str(&proof.metadata_json).unwrap();
        Artifact {
            url: format!(
                "https://github.com/{}/releases/download/{}/{}",
                metadata.source_repo, metadata.tag, metadata.artifacts[0].asset_name
            ),
            sha256: hash(bytes),
            proof: Some(proof),
        }
    }

    fn assert_no_binary_download(&self) {
        let requests = self.requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            1,
            "untrusted releases must be rejected before any executable download"
        );
        assert!(requests[0].starts_with("GET /api/agent/v1/update?download_source=github "));
    }

    async fn config(&self) -> Result<Config> {
        self.config_for_version(env!("CARGO_PKG_VERSION")).await
    }

    async fn config_for_version(&self, agent_version: &str) -> Result<Config> {
        let config = Config {
            agent_root: self.root.join("core"),
            ..Config::default()
        };
        let ops = SystemOps;
        ops.create_dir(&config.agent_root, 0o755, None).await?;
        ops.write_file(
            &config.agent_root.join("update-state.json"),
            &serde_json::to_vec(&UpgradeState {
                current: agent_version.into(),
                ..UpgradeState::default()
            })?,
            0o600,
            None,
        )
        .await?;
        Ok(config)
    }

    async fn new() -> Result<Self> {
        Self::with_version("99.0.0").await
    }

    async fn with_version(version: &str) -> Result<Self> {
        let root = std::env::temp_dir().join(format!("apsp-update-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root)?;
        let source = root.join("fixture.rs");
        std::fs::write(
            &source,
            r##"#![forbid(unsafe_code)]
fn main() {
    MARK_EXECUTED
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments == ["--version"] {
        println!("apsp-agent FIXTURE_VERSION");
    } else if arguments.len() == 3 && arguments[0] == "--config" && arguments[2] == "verify-cache" {
        if std::fs::read(&arguments[1]).unwrap() != b"accept cache" { std::process::exit(17); }
    } else {
        std::process::exit(2);
    }
}
"##
            .replace("FIXTURE_VERSION", version)
            .replace(
                "MARK_EXECUTED",
                &format!(
                    "std::fs::write({:?}, b\"executed\").unwrap();",
                    root.join("executed")
                ),
            ),
        )?;
        let binary = root.join("fixture");
        let output = tokio::process::Command::new("rustc")
            .args(["--edition=2024", "--crate-name", "update_fixture"])
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .output()
            .await?;
        ensure!(
            output.status.success(),
            "compile fixture: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = std::fs::read(binary)?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let release = Arc::new(Mutex::new(AgentRelease {
            version: version.into(),
            download_mirror: String::new(),
            artifact: Self::artifact(&origin, version, &bytes),
        }));
        let bytes = Arc::new(Mutex::new(bytes));
        let response_bytes = bytes.clone();
        let binary_path = format!(
            "GET {} ",
            reqwest::Url::parse(&release.lock().unwrap().artifact.url)?.path()
        );
        let response_release = release.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") && request.len() < 16 * 1024 {
                    match stream.read_u8().await {
                        Ok(byte) => request.push(byte),
                        Err(_) => break,
                    }
                }
                let request = String::from_utf8_lossy(&request).into_owned();
                let body =
                    if request.starts_with("GET /api/agent/v1/update?download_source=github ") {
                        serde_json::to_vec(&*response_release.lock().unwrap()).unwrap()
                    } else if request.starts_with(&binary_path) {
                        response_bytes.lock().unwrap().clone()
                    } else {
                        b"invalid executable".to_vec()
                    };
                captured.lock().unwrap().push(request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if stream.write_all(response.as_bytes()).await.is_ok() {
                    let _ = stream.write_all(&body).await;
                }
            }
        });
        Ok(Self {
            root,
            origin,
            release,
            bytes,
            requests,
            task,
        })
    }
}

#[tokio::test]
async fn independent_agent_version_can_upgrade_below_the_core_version() -> Result<()> {
    let current = "0.2.0";
    let candidate = "0.2.1";
    assert!(release_version(candidate) < release_version(env!("CARGO_PKG_VERSION")));
    let fixture = Fixture::with_version(candidate).await?;
    let config = fixture.config_for_version(current).await?;
    let mut client = PanelClient::new(&fixture.origin, "update-fixture-session")?
        .with_trusted_keys(trusted_keys());
    client.agent_download_test_origin = Some(reqwest::Url::parse(&fixture.origin)?);
    check_guarded(&config, &SystemOps, &client, current, None).await?;
    let pending: PendingUpgrade = read_json(&config.agent_root.join("pending-update.json"))?;
    assert_eq!(pending.version, candidate);
    assert!(
        config
            .agent_root
            .join(candidate)
            .join(executable_name())
            .is_file()
    );
    assert!(fixture.root.join("executed").is_file());
    assert_eq!(state(&config)?.unwrap().current, current);
    let requests = fixture.requests.lock().unwrap();
    assert!(requests[0].starts_with("GET /api/agent/v1/update?download_source=github "));
    assert!(
        requests[0]
            .to_ascii_lowercase()
            .contains("authorization: bearer update-fixture-session")
    );
    Ok(())
}

#[tokio::test]
async fn independent_agent_version_rejects_equal_or_older_candidate_above_core() -> Result<()> {
    let candidate = "99.0.0";
    assert!(release_version(candidate) > release_version(env!("CARGO_PKG_VERSION")));
    let fixture = Fixture::new().await?;
    let mut client = PanelClient::new(&fixture.origin, "update-fixture-session")?
        .with_trusted_keys(trusted_keys());
    client.agent_download_test_origin = Some(reqwest::Url::parse(&fixture.origin)?);
    for current in [candidate, "99.0.1"] {
        let config = fixture.config_for_version(current).await?;
        fixture.requests.lock().unwrap().clear();
        let error = check_guarded(&config, &SystemOps, &client, current, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("newer stable"));
        fixture.assert_no_binary_download();
        assert!(!config.agent_root.join("pending-update.json").exists());
        assert!(!fixture.root.join("executed").exists());
    }
    Ok(())
}

#[tokio::test]
async fn stages_signed_updates_only_after_identity_digest_format_and_version_checks() -> Result<()>
{
    let fixture = Fixture::new().await?;
    let config = fixture.config().await?;
    let ops = SystemOps;
    let mut client = PanelClient::new(&fixture.origin, "update-fixture-session")?
        .with_trusted_keys(trusted_keys());
    client.agent_download_test_origin = Some(reqwest::Url::parse(&fixture.origin)?);
    let original = fixture.release.lock().unwrap().clone();
    let pending = config.agent_root.join("pending-update.json");

    fixture.release.lock().unwrap().artifact.sha256 = "0".repeat(64);
    assert!(
        check(&config, &ops, &client)
            .await
            .unwrap_err()
            .to_string()
            .contains("SHA256")
    );
    assert!(!config.agent_root.join("99.0.0").exists() && !pending.exists());

    let valid_bytes = fixture.bytes.lock().unwrap().clone();
    *fixture.bytes.lock().unwrap() = b"invalid executable".to_vec();
    fixture.release.lock().unwrap().artifact =
        Fixture::artifact(&fixture.origin, "99.0.0", b"invalid executable");
    assert!(
        check(&config, &ops, &client)
            .await
            .unwrap_err()
            .to_string()
            .contains("executable format")
    );
    assert!(!pending.exists());

    *fixture.release.lock().unwrap() = original.clone();
    *fixture.bytes.lock().unwrap() = valid_bytes;
    fixture.release.lock().unwrap().artifact.url = "https://other.example/binary".into();
    assert!(
        check(&config, &ops, &client)
            .await
            .unwrap_err()
            .to_string()
            .contains("identity differs")
    );
    assert!(!pending.exists());

    *fixture.release.lock().unwrap() = original.clone();
    fixture.release.lock().unwrap().version = env!("CARGO_PKG_VERSION").into();
    assert!(
        check(&config, &ops, &client)
            .await
            .unwrap_err()
            .to_string()
            .contains("newer stable")
    );
    fixture.release.lock().unwrap().version = "99.0.1".into();
    assert!(
        check(&config, &ops, &client)
            .await
            .unwrap_err()
            .to_string()
            .contains("does not contain the requested artifact")
    );
    assert!(!pending.exists());

    *fixture.release.lock().unwrap() = original.clone();
    check(&config, &ops, &client).await?;
    for request in
        fixture.requests.lock().unwrap().iter().filter(|request| {
            !request.starts_with("GET /api/agent/v1/update?download_source=github ")
        })
    {
        assert!(!request.to_lowercase().contains("authorization:"));
        assert!(!request.contains("update-fixture-session"));
    }
    let staged: PendingUpgrade = read_json(&pending)?;
    assert_eq!(staged.version, original.version);
    assert_eq!(staged.sha256, original.artifact.sha256);
    let proof = staged
        .proof
        .as_ref()
        .context("pending upgrade has no signed proof")?;
    assert_eq!(
        proof.metadata_json,
        original.artifact.proof.as_ref().unwrap().metadata_json
    );
    for (name, expected) in [
        ("release.json", proof.metadata_json.as_bytes()),
        ("SHA256SUMS", proof.checksums.as_bytes()),
        ("SHA256SUMS.minisig", proof.signature.as_bytes()),
    ] {
        assert_eq!(
            std::fs::read(config.agent_root.join("99.0.0").join(name))?,
            expected
        );
    }
    assert_eq!(
        std::fs::metadata(&pending)?.permissions().mode() & 0o777,
        0o600
    );
    assert!(state(&config)?.unwrap().trial.is_none());
    assert!(
        fixture.root.join("executed").exists(),
        "a correctly signed candidate must still pass its CLI version check"
    );

    std::fs::remove_file(&pending)?;
    std::fs::write(
        config.agent_root.join("99.0.0").join(executable_name()),
        b"changed",
    )?;
    assert!(
        check(&config, &ops, &client)
            .await
            .unwrap_err()
            .to_string()
            .contains("different bytes")
    );
    assert!(!pending.exists());
    let mut failed = state(&config)?.unwrap();
    failed.failed(original.version);
    ops.write_file(
        &config.agent_root.join("update-state.json"),
        &serde_json::to_vec(&failed)?,
        0o600,
        None,
    )
    .await?;
    fixture.requests.lock().unwrap().clear();
    check(&config, &ops, &client).await?;
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "failed releases must not be downloaded again"
    );
    assert!(
        requests[0]
            .to_lowercase()
            .contains("authorization: bearer update-fixture-session")
    );
    assert!(!pending.exists());
    Ok(())
}

#[tokio::test]
async fn rejects_unsigned_or_corrupt_updates_before_download_or_execution() -> Result<()> {
    let fixture = Fixture::new().await?;
    let config = fixture.config().await?;
    let mut client = PanelClient::new(&fixture.origin, "update-fixture-session")?
        .with_trusted_keys(trusted_keys());
    client.agent_download_test_origin = Some(reqwest::Url::parse(&fixture.origin)?);
    let original = fixture.release.lock().unwrap().clone();
    let mut rejected = Vec::new();
    let mut unsigned = original.clone();
    unsigned.artifact.proof = None;
    rejected.push(unsigned);
    let mut invalid_signature = original.clone();
    invalid_signature
        .artifact
        .proof
        .as_mut()
        .unwrap()
        .checksums
        .push_str(&format!("{}  tampered\n", "0".repeat(64)));
    rejected.push(invalid_signature);
    let mut invalid_metadata = original.clone();
    invalid_metadata
        .artifact
        .proof
        .as_mut()
        .unwrap()
        .metadata_json
        .push(' ');
    rejected.push(invalid_metadata);
    let mut wrong_identity = original.clone();
    wrong_identity.artifact.url = format!("{}/binary", fixture.origin);
    rejected.push(wrong_identity);
    for release in rejected {
        *fixture.release.lock().unwrap() = release;
        fixture.requests.lock().unwrap().clear();
        assert!(check(&config, &SystemOps, &client).await.is_err());
        fixture.assert_no_binary_download();
        assert!(!config.agent_root.join("99.0.0").exists());
        assert!(!config.agent_root.join("pending-update.json").exists());
        assert!(
            !fixture.root.join("executed").exists(),
            "untrusted bytes must never execute"
        );
    }
    Ok(())
}

#[tokio::test]
async fn candidate_cache_preflight_uses_the_actual_configuration_and_rejects_failure() -> Result<()>
{
    let fixture = Fixture::new().await?;
    let config = fixture.config().await?;
    let mut client = PanelClient::new(&fixture.origin, "update-fixture-session")?
        .with_trusted_keys(trusted_keys());
    client.agent_download_test_origin = Some(reqwest::Url::parse(&fixture.origin)?);
    check(&config, &SystemOps, &client).await?;
    let candidate = config.agent_root.join("99.0.0").join(executable_name());
    let path = fixture.root.join("custom Agent configuration.toml");
    std::fs::write(&path, b"accept cache")?;
    verify_cache_before_upgrade(&candidate, &path, &SystemOps).await?;
    std::fs::write(&path, b"cache no longer trusted by candidate")?;
    let before = std::fs::read(config.agent_root.join("update-state.json"))?;
    let error = verify_cache_before_upgrade(&candidate, &path, &SystemOps)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cache signature preflight"));
    assert_eq!(
        std::fs::read(config.agent_root.join("update-state.json"))?,
        before,
        "cache rejection must happen before an upgrade trial is recorded"
    );
    assert_eq!(
        std::fs::read(&path)?,
        b"cache no longer trusted by candidate"
    );
    Ok(())
}

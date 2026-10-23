#![forbid(unsafe_code)]
#![cfg(unix)]

use anyhow::Result;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
use apsp_adapter_sdk::{Descriptor, Privileged};
use apsp_agent_core::{artifacts::PanelClient, system::SystemOps};
use apsp_protocol::{Artifact, Bundle};
use flate2::{Compression, write::GzEncoder};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
    task::JoinHandle,
};
use uuid::Uuid;

struct Temporary(PathBuf);

impl Temporary {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("apsp-artifacts-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct HttpServer {
    origin: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl HttpServer {
    async fn new(body: Vec<u8>, status: &str, extra_headers: &str) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n",
            body.len()
        )
        .into_bytes();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let Ok(count) = stream.read(&mut buffer).await else {
                        break;
                    };
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                }
                captured
                    .lock()
                    .await
                    .push(String::from_utf8_lossy(&request).into_owned());
                if stream.write_all(&response).await.is_ok() {
                    let _ = stream.write_all(&body).await;
                }
            }
        });
        Ok(Self {
            origin,
            requests,
            task,
        })
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn descriptor() -> Descriptor {
    Descriptor {
        auxiliary_files: vec![],
        module: "runtime".into(),
        plugin_name: "runtime".into(),
        binary_name: "runtime".into(),
        service_unit: "runtime@main.service".into(),
        service_group: String::new(),
    }
}

fn archive(entries: &[(&str, tar::EntryType, &[u8])]) -> Result<Vec<u8>> {
    let mut raw = Vec::new();
    for (path, kind, body) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o755);
        header.set_entry_type(*kind);
        header.set_size(body.len() as u64);
        let path_bytes = path.as_bytes();
        header.as_mut_bytes()[..path_bytes.len()].copy_from_slice(path_bytes);
        if kind.is_symlink() || kind.is_hard_link() {
            header.set_link_name("outside")?;
        }
        header.set_cksum();
        raw.extend_from_slice(header.as_bytes());
        raw.extend_from_slice(body);
        raw.resize(raw.len().div_ceil(512) * 512, 0);
    }
    raw.extend_from_slice(&[0; 1024]);
    let mut compressed = GzEncoder::new(Vec::new(), Compression::fast());
    compressed.write_all(&raw)?;
    Ok(compressed.finish()?)
}

#[tokio::test]
async fn download_rejects_other_origins_credentials_queries_and_redirects() -> Result<()> {
    let destination = HttpServer::new(b"private".to_vec(), "200 OK", "").await?;
    let panel = HttpServer::new(
        Vec::new(),
        "302 Found",
        &format!("Location: {}/private\r\n", destination.origin),
    )
    .await?;
    let client = PanelClient::new(&panel.origin, "device-session-test")?;
    for url in [
        format!("{}/private", destination.origin),
        format!("{}/bundle?token=test", panel.origin),
        format!("{}/bundle#fragment", panel.origin),
        panel.origin.replacen("http://", "http://other@", 1),
    ] {
        assert!(client.bundle(&url, &checksum(b"private")).await.is_err());
    }
    assert!(panel.requests.lock().await.is_empty());
    assert!(
        client
            .bundle(&format!("{}/redirect", panel.origin), &checksum(b"private"))
            .await
            .is_err()
    );
    assert!(destination.requests.lock().await.is_empty());
    let requests = panel.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .to_ascii_lowercase()
            .contains("authorization: bearer device-session-test")
    );
    Ok(())
}

#[tokio::test]
async fn bundle_hashes_original_bytes_and_rejects_unsafe_paths() -> Result<()> {
    let bytes = b"{\n  \"files\": {\"config.json\": \"{}\\n\"}\n}\n";
    let server = HttpServer::new(bytes.to_vec(), "200 OK", "").await?;
    let client = PanelClient::new(&server.origin, "test-session")?;
    let url = format!("{}/bundle", server.origin);
    assert_eq!(
        client.bundle(&url, &checksum(bytes)).await?.files["config.json"],
        "{}\n"
    );
    assert!(client.bundle(&url, &"0".repeat(64)).await.is_err());
    for name in [
        "../escape",
        "/absolute",
        "a/../escape",
        "a//b",
        "./config",
        "a\\b",
        "a/",
        "",
    ] {
        let bundle = Bundle {
            files: BTreeMap::from([(name.to_owned(), "{}".into())]),
        };
        let bytes = serde_json::to_vec(&bundle)?;
        let server = HttpServer::new(bytes.clone(), "200 OK", "").await?;
        let client = PanelClient::new(&server.origin, "test-session")?;
        assert!(
            client
                .bundle(&format!("{}/bundle", server.origin), &checksum(&bytes))
                .await
                .is_err(),
            "{name}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn manifest_uses_authenticated_panel_endpoint() -> Result<()> {
    let panel = HttpServer::new(b"{\"rev\":7,\"modules\":{}}".to_vec(), "200 OK", "").await?;
    let client = PanelClient::new(&panel.origin, "test-session")?
        .with_trusted_keys(release_support::trusted_keys());
    assert_eq!(client.manifest().await?.rev, 7);
    assert!(panel.requests.lock().await[0].starts_with("GET /api/agent/v1/manifest "));
    Ok(())
}

#[tokio::test]
async fn artifact_cache_checks_contents_and_never_replaces_existing_versions() -> Result<()> {
    let temporary = Temporary::new()?;
    let packed = archive(&[("runtime", tar::EntryType::Regular, b"original-runtime")])?;
    let panel = HttpServer::new(packed.clone(), "200 OK", "").await?;
    let client = PanelClient::new(&panel.origin, "test-session")?
        .with_trusted_keys(release_support::trusted_keys());
    let artifact = Artifact {
        url: format!(
            "{}/api/agent/v1/artifacts/runtime/1.2.3/{}",
            panel.origin,
            apsp_protocol::release::native_arch()?
        ),
        sha256: checksum(&packed),
        proof: Some(release_support::proof_for_archive(
            "runtime",
            "1.2.3",
            "runtime",
            &packed,
            b"original-runtime",
        )),
    };
    let ops = SystemOps;
    let binary = client
        .ensure_artifact(&artifact, "1.2.3", &descriptor(), &temporary.0, &ops)
        .await?;
    assert_eq!(std::fs::read(&binary)?, b"original-runtime");
    assert_eq!(
        std::fs::metadata(&binary)?.permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        client
            .ensure_artifact(&artifact, "1.2.3", &descriptor(), &temporary.0, &ops)
            .await?,
        binary
    );
    assert_eq!(panel.requests.lock().await.len(), 1);
    let mismatch = Artifact {
        sha256: "0".repeat(64),
        ..artifact.clone()
    };
    assert!(
        client
            .ensure_artifact(&mismatch, "1.2.3", &descriptor(), &temporary.0, &ops)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&binary)?, b"original-runtime");
    std::fs::write(&binary, b"tampered")?;
    assert!(
        client
            .ensure_artifact(&artifact, "1.2.3", &descriptor(), &temporary.0, &ops)
            .await
            .is_err()
    );
    assert!(
        client
            .ensure_artifact(&artifact, "../escape", &descriptor(), &temporary.0, &ops)
            .await
            .is_err()
    );
    let mut invalid = descriptor();
    invalid.plugin_name = "../escape".into();
    assert!(
        client
            .ensure_artifact(&artifact, "1.2.3", &invalid, &temporary.0, &ops)
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn artifact_checksum_failure_leaves_no_version_and_symlink_paths_are_rejected() -> Result<()>
{
    let temporary = Temporary::new()?;
    let packed = archive(&[("runtime", tar::EntryType::Regular, b"runtime")])?;
    let panel = HttpServer::new(packed.clone(), "200 OK", "").await?;
    let client = PanelClient::new(&panel.origin, "test-session")?
        .with_trusted_keys(release_support::trusted_keys());
    let mut artifact = Artifact {
        proof: Some(release_support::proof_for_archive(
            "runtime", "1.0", "runtime", &packed, b"runtime",
        )),
        url: format!(
            "{}/api/agent/v1/artifacts/runtime/1.0/{}",
            panel.origin,
            apsp_protocol::release::native_arch()?
        ),
        sha256: "0".repeat(64),
    };
    assert!(
        client
            .ensure_artifact(&artifact, "1.0", &descriptor(), &temporary.0, &SystemOps)
            .await
            .is_err()
    );
    assert!(!temporary.0.join("runtime/1.0").exists());
    artifact.sha256 = checksum(&packed);
    let outside = Temporary::new()?;
    symlink(&outside.0, temporary.0.join("runtime"))?;
    assert!(
        client
            .ensure_artifact(&artifact, "1.0", &descriptor(), &temporary.0, &SystemOps)
            .await
            .is_err()
    );
    assert!(std::fs::read_dir(&outside.0)?.next().is_none());
    Ok(())
}

#[tokio::test]
async fn archive_rejects_traversal_links_special_entries_extra_files_and_corruption() -> Result<()>
{
    let temporary = Temporary::new()?;
    let malicious = [
        archive(&[("../escape", tar::EntryType::Regular, b"bad")])?,
        archive(&[("/absolute", tar::EntryType::Regular, b"bad")])?,
        archive(&[("runtime", tar::EntryType::Symlink, b"")])?,
        archive(&[("runtime", tar::EntryType::Link, b"")])?,
        archive(&[("runtime", tar::EntryType::Fifo, b"")])?,
        archive(&[("runtime", tar::EntryType::Directory, b"")])?,
        archive(&[
            ("runtime", tar::EntryType::Regular, b"first"),
            ("runtime", tar::EntryType::Regular, b"second"),
        ])?,
        archive(&[])?,
        b"not an archive".to_vec(),
    ];
    for (index, packed) in malicious.into_iter().enumerate() {
        let input = temporary.0.join(format!("case-{index}.tar.gz"));
        std::fs::write(&input, packed)?;
        let destination = temporary.0.join(format!("version-{index}"));
        assert!(
            SystemOps
                .install_archive(&input, &destination, "runtime")
                .await
                .is_err(),
            "case {index}"
        );
        assert!(!destination.exists());
    }
    assert!(!temporary.0.join("escape").exists());
    assert!(std::fs::read_dir(&temporary.0)?.all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".unpack-")
    }));
    Ok(())
}

#[tokio::test]
async fn atomic_writes_and_links_preserve_modes_and_real_files() -> Result<()> {
    let temporary = Temporary::new()?;
    let directory = temporary.0.join("parent/private");
    SystemOps.create_dir(&directory, 0o700, None).await?;
    assert_eq!(
        std::fs::metadata(directory.parent().unwrap())?
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    let file = directory.join("state");
    SystemOps.write_file(&file, b"old", 0o600, None).await?;
    SystemOps.write_file(&file, b"new", 0o640, None).await?;
    assert_eq!(std::fs::read(&file)?, b"new");
    assert_eq!(
        std::fs::metadata(&file)?.permissions().mode() & 0o777,
        0o640
    );
    assert!(SystemOps.remove_symlink(&file).await.is_err());
    assert!(SystemOps.atomic_symlink(&file, &directory).await.is_err());
    let link = directory.join("current");
    SystemOps.atomic_symlink(&link, &file).await?;
    assert_eq!(
        std::fs::symlink_metadata(&link)?.permissions().mode() & 0o444,
        0o444,
        "runtime accounts must be able to resolve the published link"
    );
    assert_eq!(
        std::fs::metadata(&file)?.permissions().mode() & 0o777,
        0o640,
        "publishing a readable link must not change its target permissions"
    );
    SystemOps
        .atomic_symlink(&link, &directory.join("missing"))
        .await?;
    assert_eq!(std::fs::read_link(&link)?, directory.join("missing"));
    assert_eq!(
        std::fs::symlink_metadata(&link)?.permissions().mode() & 0o444,
        0o444
    );
    SystemOps.remove_symlink(&link).await?;
    SystemOps.remove_symlink(&link).await?;
    assert!(std::fs::symlink_metadata(&link).is_err());
    assert_eq!(std::fs::read(&file)?, b"new");
    Ok(())
}

#[tokio::test]
async fn archive_rejects_oversized_headers_and_truncated_gzip_streams() -> Result<()> {
    let temporary = Temporary::new()?;
    let mut header = tar::Header::new_gnu();
    header.set_path("runtime")?;
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(256 * 1024 * 1024 + 1);
    header.set_cksum();
    let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
    gzip.write_all(header.as_bytes())?;
    gzip.write_all(&[0; 1024])?;
    let oversized = gzip.finish()?;
    let mut truncated = archive(&[("runtime", tar::EntryType::Regular, b"runtime")])?;
    truncated.truncate(truncated.len() - 4);
    for (index, bytes) in [oversized, truncated].into_iter().enumerate() {
        let source = temporary.0.join(format!("invalid-{index}.gz"));
        std::fs::write(&source, bytes)?;
        assert!(
            SystemOps
                .install_archive(
                    &source,
                    &temporary.0.join(format!("version-{index}")),
                    "runtime"
                )
                .await
                .is_err()
        );
    }
    Ok(())
}

#[tokio::test]
async fn downloads_reject_oversized_content_length_before_reading_body() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 536870913\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    assert!(
        PanelClient::new(&origin, "test-session")?
            .manifest()
            .await
            .unwrap_err()
            .to_string()
            .contains("size limit")
    );
    server.await?;
    Ok(())
}

#[tokio::test]
async fn cancelled_commands_terminate_the_child_process() -> Result<()> {
    let temporary = Temporary::new()?;
    let pid_file = temporary.0.join("child.pid");
    let args = vec![
        "-c".into(),
        "echo $$ > \"$1\"; exec sleep 30".into(),
        "test-child".into(),
        pid_file.to_string_lossy().into_owned(),
    ];
    let task = tokio::spawn(async move {
        SystemOps
            .execute(std::path::Path::new("/bin/sh"), &args)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !pid_file.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    let pid = std::fs::read_to_string(pid_file)?;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while std::process::Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .output()
            .unwrap()
            .status
            .success()
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn unsigned_artifacts_and_signed_binary_mismatches_never_publish() -> Result<()> {
    let temporary = Temporary::new()?;
    let packed = archive(&[("runtime", tar::EntryType::Regular, b"unexpected-runtime")])?;
    let panel = HttpServer::new(packed.clone(), "200 OK", "").await?;
    let client = PanelClient::new(&panel.origin, "test-session")?
        .with_trusted_keys(release_support::trusted_keys());
    let mut artifact = Artifact {
        proof: None,
        url: format!(
            "{}/api/agent/v1/artifacts/runtime/1.0/{}",
            panel.origin,
            apsp_protocol::release::native_arch()?
        ),
        sha256: checksum(&packed),
    };
    assert!(
        client
            .ensure_artifact(&artifact, "1.0", &descriptor(), &temporary.0, &SystemOps)
            .await
            .is_err()
    );
    assert!(panel.requests.lock().await.is_empty());
    artifact.proof = Some(release_support::proof_for_archive(
        "runtime",
        "1.0",
        "runtime",
        &packed,
        b"expected-runtime",
    ));
    assert!(
        client
            .ensure_artifact(&artifact, "1.0", &descriptor(), &temporary.0, &SystemOps)
            .await
            .is_err()
    );
    assert_eq!(panel.requests.lock().await.len(), 1);
    assert!(!temporary.0.join("runtime/1.0").exists());
    assert!(
        std::fs::read_dir(temporary.0.join("runtime"))?
            .next()
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn legacy_local_hashes_cannot_authorize_a_changed_cached_binary() -> Result<()> {
    let temporary = Temporary::new()?;
    let packed = archive(&[("runtime", tar::EntryType::Regular, b"trusted-runtime")])?;
    let panel = HttpServer::new(packed.clone(), "200 OK", "").await?;
    let client = PanelClient::new(&panel.origin, "test-session")?
        .with_trusted_keys(release_support::trusted_keys());
    let proof = release_support::proof_for_archive(
        "runtime",
        "1.0",
        "runtime",
        &packed,
        b"trusted-runtime",
    );
    let artifact = Artifact {
        proof: Some(proof.clone()),
        url: format!(
            "{}/api/agent/v1/artifacts/runtime/1.0/{}",
            panel.origin,
            apsp_protocol::release::native_arch()?
        ),
        sha256: checksum(&packed),
    };
    let binary = client
        .ensure_artifact(&artifact, "1.0", &descriptor(), &temporary.0, &SystemOps)
        .await?;
    let directory = binary.parent().unwrap();
    for name in ["release.json", "SHA256SUMS", "SHA256SUMS.minisig"] {
        std::fs::remove_file(directory.join(name))?;
    }
    // Full signed proofs in the cache marker remain verifiable offline.
    assert_eq!(
        client
            .ensure_artifact(&artifact, "1.0", &descriptor(), &temporary.0, &SystemOps)
            .await?,
        binary
    );
    std::fs::write(&binary, b"hostile-runtime")?;
    std::fs::write(
        directory.join(".artifact.json"),
        serde_json::to_vec(
            &serde_json::json!({"archive_sha256":checksum(&packed),"binary_sha256":checksum(b"hostile-runtime")}),
        )?,
    )?;
    assert!(
        client
            .ensure_artifact(&artifact, "1.0", &descriptor(), &temporary.0, &SystemOps)
            .await
            .is_err()
    );
    std::fs::write(
        directory.join(".artifact.json"),
        serde_json::to_vec(
            &serde_json::json!({"proof":proof,"binary_sha256":checksum(b"hostile-runtime")}),
        )?,
    )?;
    assert!(
        client
            .ensure_artifact(&artifact, "1.0", &descriptor(), &temporary.0, &SystemOps)
            .await
            .is_err()
    );
    assert_eq!(panel.requests.lock().await.len(), 1);
    Ok(())
}

#[tokio::test]
async fn auxiliary_artifacts_require_signed_file_hashes_and_reject_tampering() -> Result<()> {
    let temporary = Temporary::new()?;
    let packed = archive(&[
        ("runtime", tar::EntryType::Regular, b"binary"),
        ("helper.dll", tar::EntryType::Regular, b"library"),
    ])?;
    let panel = HttpServer::new(packed.clone(), "200 OK", "").await?;
    let client = PanelClient::new(&panel.origin, "test-session")?
        .with_trusted_keys(release_support::trusted_keys());
    let mut entry =
        release_support::entry("runtime", "1.0", "runtime", "tar.gz", &packed, b"binary");
    entry.auxiliary_files.insert(
        "helper.dll".into(),
        apsp_protocol::release::ReleaseFile {
            sha256: checksum(b"library"),
            size: 7,
        },
    );
    let artifact = Artifact {
        proof: Some(release_support::signed_release(vec![(
            entry,
            packed.clone(),
        )])),
        url: format!(
            "{}/api/agent/v1/artifacts/runtime/1.0/{}",
            panel.origin,
            apsp_protocol::release::native_arch()?
        ),
        sha256: checksum(&packed),
    };
    let mut descriptor = descriptor();
    descriptor.auxiliary_files = vec!["helper.dll".into()];
    let binary = client
        .ensure_artifact(&artifact, "1.0", &descriptor, &temporary.0, &SystemOps)
        .await?;
    assert_eq!(
        std::fs::read(binary.parent().unwrap().join("helper.dll"))?,
        b"library"
    );
    client
        .ensure_artifact(&artifact, "1.0", &descriptor, &temporary.0, &SystemOps)
        .await?;
    assert_eq!(panel.requests.lock().await.len(), 1);
    std::fs::write(binary.parent().unwrap().join("helper.dll"), b"changed")?;
    assert!(
        client
            .ensure_artifact(&artifact, "1.0", &descriptor, &temporary.0, &SystemOps)
            .await
            .is_err()
    );
    assert_eq!(panel.requests.lock().await.len(), 1);
    descriptor.auxiliary_files.clear();
    assert!(
        client
            .ensure_artifact(&artifact, "1.0", &descriptor, &temporary.0, &SystemOps)
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn signed_five_file_provenance_is_installed_and_cache_tampering_is_rejected() -> Result<()> {
    let temporary = Temporary::new()?;
    let auxiliary: [(&str, &[u8]); 5] = [
        ("build-info.json", b"TEST_ONLY build provenance"),
        ("LICENSE", b"TEST_ONLY license"),
        ("source.tar.gz", b"TEST_ONLY fixed source archive"),
        ("Cargo.lock", b"TEST_ONLY locked dependencies"),
        (
            "THIRD_PARTY_NOTICES.txt",
            b"TEST_ONLY third-party originals",
        ),
    ];
    let mut members = vec![("runtime", tar::EntryType::Regular, b"binary".as_slice())];
    members.extend(
        auxiliary
            .iter()
            .map(|(name, data)| (*name, tar::EntryType::Regular, *data)),
    );
    let packed = archive(&members)?;
    let panel = HttpServer::new(packed.clone(), "200 OK", "").await?;
    let client = PanelClient::new(&panel.origin, "test-session")?
        .with_trusted_keys(release_support::trusted_keys());
    let mut entry =
        release_support::entry("runtime", "1.0", "runtime", "tar.gz", &packed, b"binary");
    for (name, content) in auxiliary {
        entry.auxiliary_files.insert(
            name.into(),
            apsp_protocol::release::ReleaseFile {
                sha256: checksum(content),
                size: content.len() as u64,
            },
        );
    }
    let artifact = Artifact {
        proof: Some(release_support::signed_release(vec![(
            entry,
            packed.clone(),
        )])),
        url: format!(
            "{}/api/agent/v1/artifacts/runtime/1.0/{}",
            panel.origin,
            apsp_protocol::release::native_arch()?
        ),
        sha256: checksum(&packed),
    };
    let mut descriptor = descriptor();
    descriptor.auxiliary_files = auxiliary
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();
    let binary = client
        .ensure_artifact(&artifact, "1.0", &descriptor, &temporary.0, &SystemOps)
        .await?;
    for (name, content) in auxiliary {
        assert_eq!(std::fs::read(binary.parent().unwrap().join(name))?, content);
        std::fs::write(binary.parent().unwrap().join(name), b"tampered")?;
        assert!(
            client
                .ensure_artifact(&artifact, "1.0", &descriptor, &temporary.0, &SystemOps)
                .await
                .is_err()
        );
        std::fs::write(binary.parent().unwrap().join(name), content)?;
    }
    assert_eq!(panel.requests.lock().await.len(), 1);
    Ok(())
}

#![forbid(unsafe_code)]
#![cfg(unix)]

use anyhow::Result;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
use apsp_adapter_sdk::Privileged;
use apsp_agent_core::{
    artifacts::PanelClient,
    bundled::{BundledItem, BundledPayload, ensure_materialized, verify_compiled_item},
    system::SystemOps,
};
use apsp_protocol::Bundle;
use flate2::{Compression, write::GzEncoder};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
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
async fn bundled_materialization_verifies_contents_preserves_rollback_dirs_and_rejects_tampering()
-> Result<()> {
    let temporary = Temporary::new()?;
    let ops = SystemOps;
    let packed_a = archive(&[
        ("runtime", tar::EntryType::Regular, b"bundled-runtime-a"),
        (
            "lib/helper.so",
            tar::EntryType::Regular,
            b"bundled-helper-a",
        ),
    ])?;
    let item_a = BundledItem {
        component_name: "runtime".into(),
        binary_name: "runtime".into(),
        version: "1.2.3+build1".into(),
        payload: BundledPayload::TarGz {
            archive_sha256: checksum(&packed_a),
            bytes: Cow::Owned(packed_a.clone()),
        },
    };
    let bin_a = ensure_materialized(&item_a, &temporary.0, &ops).await?;
    assert_eq!(bin_a, temporary.0.join("runtime/1.2.3+build1/runtime"));
    assert_eq!(std::fs::read(&bin_a)?, b"bundled-runtime-a");
    assert_eq!(
        std::fs::read(temporary.0.join("runtime/1.2.3+build1/lib/helper.so"))?,
        b"bundled-helper-a"
    );
    assert_eq!(
        std::fs::metadata(&bin_a)?.permissions().mode() & 0o777,
        0o755
    );
    // Idempotent reuse when manifest and files match.
    assert_eq!(
        ensure_materialized(&item_a, &temporary.0, &ops).await?,
        bin_a
    );

    // A newer Agent rebuild carrying the same semantic version string materializes into
    // `<version>-<hash12>` without overwriting `<version>`, keeping rollback intact.
    let packed_b = archive(&[("runtime", tar::EntryType::Regular, b"bundled-runtime-b")])?;
    let sha_b = checksum(&packed_b);
    let item_b = BundledItem {
        component_name: "runtime".into(),
        binary_name: "runtime".into(),
        version: "1.2.3+build1".into(),
        payload: BundledPayload::TarGz {
            archive_sha256: sha_b.clone(),
            bytes: Cow::Owned(packed_b),
        },
    };
    let bin_b = ensure_materialized(&item_b, &temporary.0, &ops).await?;
    assert_eq!(
        bin_b,
        temporary
            .0
            .join(format!("runtime/1.2.3+build1-{}/runtime", &sha_b[..12]))
    );
    assert_eq!(std::fs::read(&bin_a)?, b"bundled-runtime-a");
    assert_eq!(std::fs::read(&bin_b)?, b"bundled-runtime-b");

    // Tampering with an extracted file fails verification against the compiled item.
    std::fs::write(&bin_b, b"tampered-runtime")?;
    assert!(
        verify_compiled_item(&item_b, &temporary.0, &bin_b)
            .await
            .is_err()
    );

    // Mismatched archive digest or symlinked component directory is rejected without publishing.
    let bad_digest_item = BundledItem {
        component_name: "corrupt".into(),
        binary_name: "runtime".into(),
        version: "1.0.0".into(),
        payload: BundledPayload::TarGz {
            archive_sha256: "0".repeat(64),
            bytes: Cow::Owned(packed_a.clone()),
        },
    };
    assert!(
        ensure_materialized(&bad_digest_item, &temporary.0, &ops)
            .await
            .is_err()
    );
    assert!(!temporary.0.join("corrupt/1.0.0").exists());

    let outside = Temporary::new()?;
    symlink(&outside.0, temporary.0.join("escaped"))?;
    let symlink_item = BundledItem {
        component_name: "escaped".into(),
        binary_name: "runtime".into(),
        version: "1.0.0".into(),
        payload: BundledPayload::TarGz {
            archive_sha256: checksum(&packed_a),
            bytes: Cow::Owned(packed_a),
        },
    };
    assert!(
        ensure_materialized(&symlink_item, &temporary.0, &ops)
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

use super::*;
use crate::{release_test_support as release_support, system::SystemOps};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

const BINARY: &[u8] = b"TEST_ONLY streamed runtime fixture";
const SESSION: &str = "TEST_ONLY_fixture_session";

struct Directory(PathBuf);

impl Directory {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("apsp-cache-stream-{}", Uuid::new_v4()));
        std::fs::create_dir(&path)?;
        let directory = Self(path);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory.0, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(directory)
    }

    fn install_root(&self) -> PathBuf {
        self.0.join("install")
    }

    fn plugin(&self) -> PathBuf {
        self.install_root().join("runtime-fixture")
    }

    fn assert_no_private_downloads(&self) -> Result<()> {
        for entry in std::fs::read_dir(self.plugin())? {
            let name = entry?.file_name();
            assert!(!name.to_string_lossy().starts_with('.'));
        }
        Ok(())
    }

    fn assert_not_installed(&self) -> Result<()> {
        self.assert_no_private_downloads()?;
        assert!(!self.plugin().join("v1").exists());
        assert_eq!(std::fs::read_dir(self.plugin())?.count(), 0);
        Ok(())
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

enum Reply {
    Chunked(Vec<u8>),
    BrokenChunked(Vec<u8>),
    Declared { length: u64, body: Vec<u8> },
    Stalled(Vec<u8>),
    Redirect,
}

struct Request {
    headers: String,
    disconnected: bool,
}

struct HttpFixture {
    base: String,
    task: Option<JoinHandle<Result<Request>>>,
}

impl HttpFixture {
    async fn start(reply: Reply) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut headers = Vec::new();
            loop {
                let mut buffer = [0_u8; 1024];
                let count = socket.read(&mut buffer).await?;
                ensure!(count > 0, "fixture request ended before its headers");
                ensure!(
                    headers.len() + count <= 8192,
                    "fixture request is too large"
                );
                headers.extend_from_slice(&buffer[..count]);
                if headers.windows(4).any(|value| value == b"\r\n\r\n") {
                    break;
                }
            }
            let headers = String::from_utf8(headers)?;
            let complete = matches!(&reply, Reply::Chunked(_));
            let stalled = matches!(&reply, Reply::Stalled(_));
            match reply {
                Reply::Chunked(body) | Reply::BrokenChunked(body) | Reply::Stalled(body) => {
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")
                        .await?;
                    for chunk in body.chunks(7) {
                        socket
                            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                            .await?;
                        socket.write_all(chunk).await?;
                        socket.write_all(b"\r\n").await?;
                    }
                    socket.flush().await?;
                }
                Reply::Declared { length, body } => {
                    socket
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
                            )
                            .as_bytes(),
                        )
                        .await?;
                    socket.write_all(&body).await?;
                    socket.shutdown().await?;
                    return Ok(Request {
                        headers,
                        disconnected: false,
                    });
                }
                Reply::Redirect => {
                    socket
                        .write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/outside\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await?;
                    socket.shutdown().await?;
                    return Ok(Request {
                        headers,
                        disconnected: false,
                    });
                }
            }
            if stalled {
                let mut extra = [0_u8; 1];
                let closed = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut extra))
                    .await
                    .context("cancelled fixture connection was not closed")?;
                match closed {
                    Ok(0) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionAborted
                        ) => {}
                    Ok(_) => anyhow::bail!("fixture connection received unexpected data"),
                    Err(error) => return Err(error.into()),
                }
                return Ok(Request {
                    headers,
                    disconnected: true,
                });
            }
            if complete {
                socket.write_all(b"0\r\n\r\n").await?;
            }
            socket.shutdown().await?;
            Ok(Request {
                headers,
                disconnected: false,
            })
        });
        Ok(Self {
            base,
            task: Some(task),
        })
    }

    async fn finish(&mut self) -> Result<Request> {
        let task = self
            .task
            .as_mut()
            .context("fixture server already joined")?;
        let result = tokio::time::timeout(Duration::from_secs(3), &mut *task).await;
        match result {
            Ok(result) => {
                let _ = self.task.take();
                result?
            }
            Err(error) => {
                task.abort();
                Err(error).context("fixture server did not finish")
            }
        }
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

struct AbortDownload(tokio::task::AbortHandle);

impl Drop for AbortDownload {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn descriptor() -> Descriptor {
    Descriptor {
        module: "runtime-fixture".into(),
        plugin_name: "runtime-fixture".into(),
        binary_name: "runner".into(),
        auxiliary_files: Vec::new(),
        service_unit: String::new(),
        service_group: String::new(),
    }
}

fn archive() -> Result<Vec<u8>> {
    let compressed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(compressed);
    let mut header = tar::Header::new_gnu();
    header.set_size(BINARY.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder.append_data(&mut header, "runner", BINARY)?;
    Ok(builder.into_inner()?.finish()?)
}

fn fixture_artifact(base: &str, archive: &[u8]) -> Result<Artifact> {
    Ok(Artifact {
        url: format!(
            "{base}/api/agent/v1/artifacts/runtime-fixture/v1/{}",
            apsp_protocol::release::native_arch()?
        ),
        sha256: release_support::hash(archive),
        proof: Some(release_support::proof_for_archive(
            "runtime-fixture",
            "v1",
            "runner",
            archive,
            BINARY,
        )),
    })
}

fn fixture_client(base: &str) -> Result<PanelClient> {
    Ok(PanelClient::new(base, SESSION)?.with_trusted_keys(release_support::trusted_keys()))
}

fn assert_request(request: &Request, artifact: &Artifact) -> Result<()> {
    let path = Url::parse(&artifact.url)?.path().to_owned();
    assert!(
        request
            .headers
            .starts_with(&format!("GET {path} HTTP/1.1\r\n"))
    );
    assert!(request.headers.to_ascii_lowercase().contains(&format!(
        "authorization: bearer {}\r\n",
        SESSION.to_ascii_lowercase()
    )));
    Ok(())
}

#[tokio::test]
async fn chunked_signed_archive_streams_to_an_installed_verified_cache() -> Result<()> {
    let directory = Directory::new()?;
    let bytes = archive()?;
    let mut server = HttpFixture::start(Reply::Chunked(bytes.clone())).await?;
    let artifact = fixture_artifact(&server.base, &bytes)?;
    let binary = fixture_client(&server.base)?
        .ensure_artifact(
            &artifact,
            "v1",
            &descriptor(),
            &directory.install_root(),
            &SystemOps,
        )
        .await?;
    assert_eq!(std::fs::read(&binary)?, BINARY);
    let request = server.finish().await?;
    assert_request(&request, &artifact)?;
    directory.assert_no_private_downloads()?;
    // A verified cache hit succeeds without starting another fixture listener.
    assert_eq!(
        fixture_client(&server.base)?
            .ensure_artifact(
                &artifact,
                "v1",
                &descriptor(),
                &directory.install_root(),
                &SystemOps
            )
            .await?,
        binary
    );
    Ok(())
}

#[tokio::test]
async fn invalid_streams_never_publish_or_leave_a_private_download() -> Result<()> {
    let bytes = archive()?;
    let mut modified = bytes.clone();
    modified[0] ^= 1;
    let mut oversized = bytes.clone();
    oversized.push(0);
    for (reply, expected_error) in [
        (
            Reply::Chunked(bytes[..bytes.len() - 1].to_vec()),
            Some("truncated"),
        ),
        (
            Reply::Chunked(oversized),
            Some("exceeds signed archive size"),
        ),
        (Reply::Chunked(modified), Some("SHA256 differs")),
        (Reply::BrokenChunked(bytes.clone()), None),
        (
            Reply::Declared {
                length: bytes.len() as u64 + 1,
                body: Vec::new(),
            },
            Some("Content-Length differs"),
        ),
        (
            Reply::Declared {
                length: MAX_DOWNLOAD as u64 + 1,
                body: Vec::new(),
            },
            Some("Content-Length differs"),
        ),
        (Reply::Redirect, Some("HTTP 302")),
    ] {
        let directory = Directory::new()?;
        let mut server = HttpFixture::start(reply).await?;
        let artifact = fixture_artifact(&server.base, &bytes)?;
        let error = fixture_client(&server.base)?
            .ensure_artifact(
                &artifact,
                "v1",
                &descriptor(),
                &directory.install_root(),
                &SystemOps,
            )
            .await
            .unwrap_err();
        if let Some(expected) = expected_error {
            assert!(error.to_string().contains(expected), "{error:#}");
        }
        assert_request(&server.finish().await?, &artifact)?;
        directory.assert_not_installed()?;
    }
    Ok(())
}

async fn wait_for_partial_download(directory: &Directory) -> Result<PathBuf> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        match std::fs::read_dir(directory.plugin()) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    if entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".download-")
                        && entry.metadata()?.len() > 0
                    {
                        return Ok(entry.path());
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "partial download was not written"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn assert_private_partial(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    assert!(metadata.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }
    Ok(())
}

#[tokio::test]
async fn download_timeout_removes_a_written_partial_and_closes_its_response() -> Result<()> {
    let directory = Directory::new()?;
    let bytes = archive()?;
    let mut server = HttpFixture::start(Reply::Stalled(bytes[..7].to_vec())).await?;
    let artifact = fixture_artifact(&server.base, &bytes)?;
    let client = fixture_client(&server.base)?;
    let descriptor = descriptor();
    let install_root = directory.install_root();
    let mut download =
        Box::pin(client.ensure_artifact(&artifact, "v1", &descriptor, &install_root, &SystemOps));
    let partial = tokio::select! {
        result = &mut download => {
            anyhow::bail!("stalled download unexpectedly finished: {result:?}");
        }
        result = wait_for_partial_download(&directory) => result?,
    };
    assert_private_partial(&partial)?;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), download)
            .await
            .is_err()
    );
    assert!(!partial.exists());
    directory.assert_not_installed()?;
    let request = server.finish().await?;
    assert!(request.disconnected);
    assert_request(&request, &artifact)?;
    Ok(())
}

#[tokio::test]
async fn cancelling_a_download_removes_its_partial_before_another_install() -> Result<()> {
    let directory = Directory::new()?;
    let bytes = archive()?;
    let mut server = HttpFixture::start(Reply::Stalled(bytes[..7].to_vec())).await?;
    let artifact = fixture_artifact(&server.base, &bytes)?;
    let install_root = directory.install_root();
    let client = fixture_client(&server.base)?;
    let requested = artifact.clone();
    let downloading = tokio::spawn(async move {
        client
            .ensure_artifact(&requested, "v1", &descriptor(), &install_root, &SystemOps)
            .await
    });
    let _abort_download = AbortDownload(downloading.abort_handle());
    let partial = wait_for_partial_download(&directory).await?;
    assert_private_partial(&partial)?;
    downloading.abort();
    assert!(downloading.await.unwrap_err().is_cancelled());
    assert!(!partial.exists());
    directory.assert_not_installed()?;
    let request = server.finish().await?;
    assert!(request.disconnected);
    assert_request(&request, &artifact)?;

    let mut retry = HttpFixture::start(Reply::Chunked(bytes.clone())).await?;
    let artifact = fixture_artifact(&retry.base, &bytes)?;
    let installed = fixture_client(&retry.base)?
        .ensure_artifact(
            &artifact,
            "v1",
            &descriptor(),
            &directory.install_root(),
            &SystemOps,
        )
        .await?;
    assert_eq!(std::fs::read(installed)?, BINARY);
    assert_request(&retry.finish().await?, &artifact)?;
    directory.assert_no_private_downloads()?;
    Ok(())
}

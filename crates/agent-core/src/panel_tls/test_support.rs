use anyhow::{Context, Result, ensure};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
    task::JoinHandle,
    time::timeout,
};
use tokio_rustls::TlsAcceptor;

pub(crate) enum CertificateKind {
    Valid,
    WrongName,
    Expired,
}

pub(crate) struct Material {
    pub root: PathBuf,
    pub ca: PathBuf,
}

async fn openssl(root: &Path, arguments: &[&str]) -> Result<()> {
    let mut command = tokio::process::Command::new("openssl");
    let mut child = command
        .args(arguments)
        .current_dir(root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("OpenSSL is required for TEST_ONLY TLS certificates")?;
    let status = match timeout(Duration::from_secs(5), child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            let _ = timeout(Duration::from_secs(2), child.kill()).await;
            let _ = timeout(Duration::from_secs(2), child.wait()).await;
            anyhow::bail!("TEST_ONLY TLS certificate generation exceeded its deadline");
        }
    };
    ensure!(
        status.success(),
        "TEST_ONLY TLS certificate generation failed"
    );
    Ok(())
}

impl Material {
    pub async fn new() -> Result<Self> {
        let path =
            std::env::temp_dir().join(format!("apsp-panel-CA-TEST_ONLY-{}", uuid::Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        let root = std::fs::canonicalize(path)?;
        let material = Self {
            ca: root.join("ca.pem"),
            root,
        };
        openssl(
            &material.root,
            &[
                "req",
                "-x509",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:prime256v1",
                "-nodes",
                "-days",
                "2",
                "-subj",
                "/CN=Apsp TEST_ONLY private panel CA",
                "-addext",
                "basicConstraints=critical,CA:TRUE",
                "-addext",
                "keyUsage=critical,keyCertSign,cRLSign",
                "-keyout",
                "ca.key",
                "-out",
                "ca.pem",
            ],
        )
        .await?;
        openssl(
            &material.root,
            &[
                "req",
                "-new",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:prime256v1",
                "-nodes",
                "-subj",
                "/CN=Apsp TEST_ONLY panel",
                "-keyout",
                "leaf.key",
                "-out",
                "leaf.csr",
            ],
        )
        .await?;
        std::fs::write(
            material.root.join("valid.ext"),
            "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1,DNS:localhost\n",
        )?;
        std::fs::write(
            material.root.join("wrong.ext"),
            "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:wrong-TEST_ONLY.example.invalid\n",
        )?;
        for (name, extensions) in [("valid.pem", "valid.ext"), ("wrong.pem", "wrong.ext")] {
            openssl(
                &material.root,
                &[
                    "x509",
                    "-req",
                    "-in",
                    "leaf.csr",
                    "-CA",
                    "ca.pem",
                    "-CAkey",
                    "ca.key",
                    "-CAcreateserial",
                    "-days",
                    "1",
                    "-extfile",
                    extensions,
                    "-out",
                    name,
                ],
            )
            .await?;
        }
        std::fs::write(material.root.join("index"), "")?;
        std::fs::write(material.root.join("serial"), "1000\n")?;
        std::fs::create_dir(material.root.join("issued"))?;
        std::fs::write(
            material.root.join("expired.cnf"),
            "[ca]\ndefault_ca=local\n[local]\ndatabase=index\nserial=serial\nnew_certs_dir=issued\ncertificate=ca.pem\nprivate_key=ca.key\ndefault_md=sha256\npolicy=policy\nx509_extensions=leaf\n[policy]\ncommonName=supplied\n[leaf]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1,DNS:localhost\n",
        )?;
        openssl(
            &material.root,
            &[
                "ca",
                "-batch",
                "-notext",
                "-config",
                "expired.cnf",
                "-startdate",
                "20000101000000Z",
                "-enddate",
                "20010101000000Z",
                "-in",
                "leaf.csr",
                "-out",
                "expired.pem",
            ],
        )
        .await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for entry in std::fs::read_dir(&material.root)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(0o600))?;
                }
            }
        }
        Ok(material)
    }

    pub fn acceptor(&self, kind: &CertificateKind) -> Result<TlsAcceptor> {
        let name = match kind {
            CertificateKind::Valid => "valid.pem",
            CertificateKind::WrongName => "wrong.pem",
            CertificateKind::Expired => "expired.pem",
        };
        let certificate = std::fs::read(self.root.join(name))?;
        let key = std::fs::read(self.root.join("leaf.key"))?;
        let certificates = CertificateDer::pem_slice_iter(&certificate)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let key = PrivateKeyDer::from_pem_slice(&key)?;
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certificates, key)?;
        Ok(TlsAcceptor::from(Arc::new(config)))
    }

    pub fn config(&self, origin: &str) -> crate::Config {
        crate::Config {
            panel_url: origin.into(),
            panel_ca_file: Some(self.ca.clone()),
            identity_dir: self.root.join("identity"),
            state_db: self.root.join("state.db"),
            runtime_root: self.root.join("runtime"),
            install_root: self.root.join("install"),
            agent_root: self.root.join("core"),
            status_socket: self.root.join("status.sock"),
            operation_timeout_secs: 3,
            ..Default::default()
        }
    }
}
impl Drop for Material {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub(crate) struct HttpRequest {
    pub target: String,
    pub headers: String,
    pub body: Vec<u8>,
}
pub(crate) struct HttpServer {
    pub origin: String,
    requests: mpsc::Receiver<HttpRequest>,
    task: JoinHandle<()>,
}
impl HttpServer {
    pub async fn start(
        material: &Material,
        kind: CertificateKind,
        status: u16,
        body: Vec<u8>,
    ) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("https://{}", listener.local_addr()?);
        let acceptor = material.acceptor(&kind)?;
        let (sender, requests) = mpsc::channel(8);
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let Ok(Ok(mut stream)) =
                    timeout(Duration::from_secs(3), acceptor.accept(socket)).await
                else {
                    continue;
                };
                let request = timeout(Duration::from_secs(3), async {
                    let mut bytes = Vec::new();
                    let mut buffer = [0u8;1024];
                    let split = loop {
                        let length = stream.read(&mut buffer).await?;
                        ensure!(length>0, "TEST_ONLY HTTP peer closed");
                        bytes.extend_from_slice(&buffer[..length]);
                        ensure!(bytes.len()<=64*1024, "TEST_ONLY HTTP request budget exceeded");
                        if let Some(split) = bytes.windows(4).position(|item| item==b"\r\n\r\n") { break split+4; }
                    };
                    let headers = std::str::from_utf8(&bytes[..split])?.to_owned();
                    let target = headers.lines().next().and_then(|line|line.split_whitespace().nth(1)).context("TEST_ONLY HTTP target missing")?.to_owned();
                    let length = headers.lines().find_map(|line| {
                        let (name,value)=line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length").then(||value.trim().parse::<usize>())
                    }).transpose()?.unwrap_or(0);
                    ensure!(length<=32*1024, "TEST_ONLY HTTP body budget exceeded");
                    while bytes.len()<split+length {
                        let count=stream.read(&mut buffer).await?;
                        ensure!(count>0, "TEST_ONLY HTTP body truncated");
                        bytes.extend_from_slice(&buffer[..count]);
                        ensure!(bytes.len()<=64*1024, "TEST_ONLY HTTP request budget exceeded");
                    }
                    let request=HttpRequest{target,headers,body:bytes[split..split+length].to_vec()};
                    stream.write_all(format!("HTTP/1.1 {status} TEST_ONLY\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await?;
                    stream.write_all(&body).await?;
                    stream.shutdown().await?;
                    Ok::<_,anyhow::Error>(request)
                }).await;
                if let Ok(Ok(request)) = request
                    && sender.send(request).await.is_err()
                {
                    break;
                }
            }
        });
        Ok(Self {
            origin,
            requests,
            task,
        })
    }
    pub async fn next(&mut self) -> Result<HttpRequest> {
        timeout(Duration::from_secs(3), self.requests.recv())
            .await?
            .context("TEST_ONLY HTTP server stopped")
    }
    pub fn has_pending_request(&mut self) -> bool {
        self.requests.try_recv().is_ok()
    }
    pub async fn stop(mut self) -> Result<()> {
        self.task.abort();
        // Join by borrowing: Drop still owns the cancellation fallback.
        match (&mut self.task).await {
            Err(error) if error.is_cancelled() => Ok(()),
            Ok(()) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}
impl Drop for HttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

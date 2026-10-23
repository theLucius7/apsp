use super::*;
use quinn::{AsyncUdpSocket, Runtime};
use serde_json::json;
use std::{
    future::poll_fn,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::time::timeout;

// These tests close an ephemeral UDP socket and immediately bind its old
// address to prove descriptor release. Keep the whole lifecycle exclusive so
// another health fixture cannot acquire that address between those operations.
// A leaked descriptor still fails the single bind; there is no retry or yield.
static UDP_LIFECYCLE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct CertificateDirectory(std::path::PathBuf);

impl CertificateDirectory {
    async fn new() -> Result<(Self, String, String)> {
        let path =
            std::env::temp_dir().join(format!("apsp-health-TEST_ONLY-{}", uuid::Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        let directory = Self(path);
        let certificate = directory.0.join("certificate.pem");
        let key = directory.0.join("key.pem");
        let mut child = tokio::process::Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-days",
                "1",
                "-subj",
                "/CN=fixture.example.com",
                "-addext",
                "subjectAltName=DNS:fixture.example.com",
                "-addext",
                "basicConstraints=critical,CA:FALSE",
                "-keyout",
            ])
            .arg(&key)
            .arg("-out")
            .arg(&certificate)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("OpenSSL is required for the TEST_ONLY QUIC certificate")?;
        let status = match timeout(Duration::from_secs(5), child.wait()).await {
            Ok(status) => status?,
            Err(error) => {
                timeout(Duration::from_secs(2), child.kill()).await??;
                timeout(Duration::from_secs(2), child.wait()).await??;
                return Err(error.into());
            }
        };
        anyhow::ensure!(status.success(), "TEST_ONLY certificate generation failed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&certificate, &key] {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            }
        }
        Ok((
            directory,
            std::fs::read_to_string(certificate)?,
            std::fs::read_to_string(key)?,
        ))
    }
}

impl Drop for CertificateDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_released(addresses: impl IntoIterator<Item = SocketAddr>) {
    // Keep every rebound socket until all addresses have been checked. This
    // also proves that distinct fixture descriptors did not share an address.
    let _rebound: Vec<_> = addresses
        .into_iter()
        .map(|address| {
            std::net::UdpSocket::bind(address)
                .unwrap_or_else(|error| panic!("health socket {address} remained bound: {error}"))
        })
        .collect();
}

fn resources() -> Result<socket::Resources> {
    let socket = socket::Socket::bind("127.0.0.1:0".parse()?)?;
    let runtime = Arc::new(socket::Runtime::default());
    let mut endpoint = quinn::Endpoint::new_with_abstract_socket(
        Default::default(),
        None,
        socket.clone(),
        runtime.clone(),
    )?;
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    endpoint.set_default_client_config(quinn::ClientConfig::with_root_certificates(Arc::new(
        roots,
    ))?);
    Ok(socket::Resources {
        endpoint,
        socket,
        runtime,
    })
}

#[tokio::test]
async fn pure_quinn_loopback_validates_certificate_and_custom_alpn() -> Result<()> {
    use rustls::pki_types::PrivateKeyDer;
    let _exclusive_sockets = UDP_LIFECYCLE.lock().await;
    let (_directory, certificate, key) = CertificateDirectory::new().await?;
    let chain = CertificateDer::pem_slice_iter(certificate.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(chain, PrivateKeyDer::from_pem_slice(key.as_bytes())?)?;
    tls.alpn_protocols = vec![b"apsp-TEST_ONLY-health".to_vec()];
    let config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(tls)?,
    ));
    let socket = socket::Socket::bind("127.0.0.1:0".parse()?)?;
    let address = socket.local_addr()?;
    let runtime = Arc::new(socket::Runtime::default());
    let endpoint = quinn::Endpoint::new_with_abstract_socket(
        Default::default(),
        Some(config),
        socket.clone(),
        runtime.clone(),
    )?;
    let server = socket::Resources {
        endpoint,
        socket,
        runtime,
    };
    let endpoint = server.endpoint.clone();
    server.runtime.spawn(Box::pin(async move {
        while let Some(incoming) = endpoint.accept().await {
            // A failed test handshake is expected for the two negative cases.
            // This accept loop belongs to the same explicitly joined scope.
            drop(incoming.await);
        }
    }));
    let mut listener = Listener {
        address,
        transport: Transport::Udp,
        obfuscation: None,
        tls: Some(
            json!({"server_name":"fixture.example.com", "certificate":certificate,
            "alpn":["apsp-TEST_ONLY-health"]}),
        ),
    };
    let result = async {
        anyhow::ensure!(
            probe(&listener).await,
            "valid custom-ALPN QUIC handshake failed"
        );
        listener
            .tls
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("certificate");
        anyhow::ensure!(
            !probe(&listener).await,
            "untrusted TEST_ONLY certificate was accepted"
        );
        listener.tls.as_mut().unwrap()["certificate"] = json!(certificate);
        listener.tls.as_mut().unwrap()["alpn"] = json!(["wrong-TEST_ONLY-alpn"]);
        anyhow::ensure!(!probe(&listener).await, "incorrect ALPN was accepted");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    timeout(Duration::from_secs(1), server.finish()).await??;
    assert!(server.socket.is_closed());
    assert_released([address]);
    result
}

#[tokio::test]
async fn owned_quic_socket_transfers_datagrams_and_closes_retained_pollers() -> Result<()> {
    let _exclusive_sockets = UDP_LIFECYCLE.lock().await;
    let socket = socket::Socket::bind("127.0.0.1:0".parse()?)?;
    let address = socket.local_addr()?;
    let peer = UdpSocket::bind("127.0.0.1:0").await?;
    let mut poller = socket.clone().create_io_poller();
    timeout(
        Duration::from_secs(1),
        poll_fn(|cx| poller.as_mut().poll_writable(cx)),
    )
    .await??;
    socket.try_send(&quinn::udp::Transmit {
        destination: peer.local_addr()?,
        contents: b"test-only",
        ecn: None,
        segment_size: None,
        src_ip: None,
    })?;
    let mut bytes = [0; 64];
    let (length, sender) = timeout(Duration::from_secs(1), peer.recv_from(&mut bytes)).await??;
    assert_eq!(&bytes[..length], b"test-only");
    assert_eq!(sender, address);
    peer.send_to(b"reply", address).await?;
    let mut metadata = [quinn::udp::RecvMeta::default()];
    let count = timeout(
        Duration::from_secs(1),
        poll_fn(|cx| {
            socket.poll_recv(
                cx,
                &mut [std::io::IoSliceMut::new(&mut bytes)],
                &mut metadata,
            )
        }),
    )
    .await??;
    assert_eq!(count, 1);
    assert_eq!(&bytes[..metadata[0].len], b"reply");
    assert_eq!(metadata[0].addr, peer.local_addr()?);
    socket.close();
    assert!(socket.is_closed());
    assert_released([address]);
    assert!(
        poll_fn(|cx| poller.as_mut().poll_writable(cx))
            .await
            .is_err()
    );
    assert!(socket.local_addr().is_err());
    Ok(())
}

#[tokio::test]
async fn quic_error_and_timeout_explicitly_join_owned_drivers() -> Result<()> {
    let _exclusive_sockets = UDP_LIFECYCLE.lock().await;
    for invalid_name in [true, false] {
        let resources = resources()?;
        let address = resources.socket.local_addr()?;
        let peer = UdpSocket::bind("127.0.0.1:0").await?;
        let connecting = resources.endpoint.connect(
            peer.local_addr()?,
            if invalid_name {
                "invalid name"
            } else {
                "fixture.example.com"
            },
        );
        if invalid_name {
            assert!(connecting.is_err());
        } else {
            let connecting = connecting?;
            let mut packet = [0; 2048];
            let handshake = async {
                let (length, _) = peer.recv_from(&mut packet).await?;
                assert!(length > 0);
                connecting.await.map_err(anyhow::Error::from)
            };
            assert!(timeout(Duration::from_millis(50), handshake).await.is_err());
        }
        timeout(Duration::from_secs(1), resources.finish()).await??;
        assert!(resources.socket.is_closed());
        assert_released([address]);
    }
    Ok(())
}

#[tokio::test]
async fn salamander_loopback_echo_and_timeout_release_both_sockets() -> Result<()> {
    let _exclusive_sockets = UDP_LIFECYCLE.lock().await;
    let peer = UdpSocket::bind("127.0.0.1:0").await?;
    let bridge =
        crate::obfuscation::Bridge::new(peer.local_addr()?, "TEST_ONLY_salamander".into()).await?;
    let addresses = bridge.addresses()?;
    let client = UdpSocket::bind("127.0.0.1:0").await?;
    let exchange = async {
        client
            .send_to(b"bounded-test-payload", bridge.address)
            .await?;
        let mut framed = [0; 128];
        let (length, source) = peer.recv_from(&mut framed).await?;
        assert_eq!(length, b"bounded-test-payload".len() + 8);
        peer.send_to(&framed[..length], source).await?;
        let mut decoded = [0; 128];
        let (length, _) = client.recv_from(&mut decoded).await?;
        assert_eq!(&decoded[..length], b"bounded-test-payload");
        Ok::<_, anyhow::Error>(())
    };
    timeout(Duration::from_secs(1), async {
        tokio::select! {
            result = exchange => result,
            result = bridge.forward() => {
                result?;
                anyhow::bail!("bridge unexpectedly finished");
            }
        }
    })
    .await??;
    bridge.close();
    assert_released(addresses);

    let bridge =
        crate::obfuscation::Bridge::new(peer.local_addr()?, "TEST_ONLY_salamander".into()).await?;
    let addresses = bridge.addresses()?;
    assert!(
        timeout(Duration::from_millis(10), bridge.forward())
            .await
            .is_err()
    );
    bridge.close();
    assert_released(addresses);
    Ok(())
}

#[tokio::test]
async fn cancelling_the_probe_closes_quic_and_bridge_sockets_without_a_retry() -> Result<()> {
    let _exclusive_sockets = UDP_LIFECYCLE.lock().await;
    let peer = UdpSocket::bind("127.0.0.1:0").await?;
    let resources = resources()?;
    let retained = resources.socket.clone();
    let mut retained_poller = retained.clone().create_io_poller();
    let address = retained.local_addr()?;
    let bridge =
        crate::obfuscation::Bridge::new(peer.local_addr()?, "TEST_ONLY_salamander".into()).await?;
    let bridge_addresses = bridge.addresses()?;
    let (started, ready) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let resources = resources;
        let bridge = bridge;
        let connecting = resources
            .endpoint
            .connect(bridge.address, "fixture.example.com")
            .unwrap();
        started.send(()).unwrap();
        tokio::select! {
            result = connecting => { drop(result); }
            result = bridge.forward() => { drop(result); }
        }
    });
    ready.await?;
    let mut packet = [0; 2048];
    let (length, _) = timeout(Duration::from_secs(1), peer.recv_from(&mut packet)).await??;
    assert!(length > 8);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(retained.is_closed());
    assert_eq!(
        retained.local_addr().unwrap_err().kind(),
        ErrorKind::NotConnected
    );
    assert_eq!(
        poll_fn(|cx| retained_poller.as_mut().poll_writable(cx))
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::NotConnected
    );
    assert_released(std::iter::once(address).chain(bridge_addresses));
    Ok(())
}

#[tokio::test]
async fn driver_cleanup_waits_for_all_tasks_and_refuses_late_spawns() -> Result<()> {
    struct Active(Arc<AtomicUsize>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let runtime = socket::Runtime::default();
    let active = Arc::new(AtomicUsize::new(2));
    for _ in 0..2 {
        let owner = Active(active.clone());
        runtime.spawn(Box::pin(async move {
            let _owner = owner;
            std::future::pending::<()>().await;
        }));
    }
    timeout(Duration::from_secs(1), runtime.finish()).await??;
    assert_eq!(active.load(Ordering::SeqCst), 0);
    let active = Arc::new(AtomicUsize::new(1));
    let owner = Active(active.clone());
    runtime.spawn(Box::pin(async move {
        let _owner = owner;
        std::future::pending::<()>().await;
    }));
    assert_eq!(active.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
#[ignore = "requires OpenSSL and official sing-box 1.14.2 in APSP_TEST_UPSTREAM; runs only on ephemeral loopback ports"]
async fn obfuscated_quic_health_checks_certificate_and_custom_alpn() -> Result<()> {
    let _exclusive_sockets = UDP_LIFECYCLE.lock().await;
    let binary = std::env::var("APSP_TEST_UPSTREAM")?;
    let directory = std::env::temp_dir().join(format!("apsp-health-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory)?;
    let certificate = directory.join("certificate.pem");
    let key = directory.join("key.pem");
    let output = std::process::Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-subj",
            "/CN=proxy.example.com",
            "-addext",
            "subjectAltName=DNS:proxy.example.com",
            "-addext",
            "basicConstraints=critical,CA:FALSE",
            "-keyout",
        ])
        .arg(&key)
        .arg("-out")
        .arg(&certificate)
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "test certificate generation failed"
    );
    let certificate = std::fs::read_to_string(certificate)?;
    let key = std::fs::read_to_string(key)?;
    let reserved = UdpSocket::bind("127.0.0.1:0").await?;
    let address = reserved.local_addr()?;
    drop(reserved);
    let password = "synthetic-test-obfuscation";
    let tls = json!({"enabled":true,"server_name":"proxy.example.com","certificate":certificate,"key":key,"alpn":["apsp-test"]});
    let path = directory.join("config.json");
    std::fs::write(&path, json!({"log":{"disabled":true},"inbounds":[{
        "type":"hysteria2","listen":"127.0.0.1","listen_port":address.port(),"users":[{"name":"test","password":"synthetic-test-credential"}],
        "obfs":{"type":"salamander","password":password},"tls":tls
    }],"outbounds":[{"type":"direct"}]}).to_string())?;
    let mut child = tokio::process::Command::new(binary)
        .arg("run")
        .arg("-c")
        .arg(path)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let mut listener = Listener {
        address,
        transport: Transport::Udp,
        tls: Some(tls),
        obfuscation: Some(password.into()),
    };
    let result = async {
        timeout(Duration::from_secs(10), async {
            loop {
                if probe(&listener).await {
                    break;
                }
                anyhow::ensure!(child.try_wait()?.is_none(), "test runtime exited");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        listener.obfuscation = Some("wrong-synthetic-password".into());
        anyhow::ensure!(
            !probe(&listener).await,
            "incorrect obfuscation was accepted"
        );
        listener.obfuscation = Some(password.into());
        listener.tls.as_mut().unwrap()["server_name"] = json!("wrong.example.com");
        anyhow::ensure!(
            !probe(&listener).await,
            "incorrect certificate name was accepted"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    child.kill().await?;
    child.wait().await?;
    std::fs::remove_dir_all(directory)?;
    result
}

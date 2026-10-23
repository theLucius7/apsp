#![forbid(unsafe_code)]

#[path = "../../adapter-singbox/tests/support/mod.rs"]
#[allow(dead_code)]
mod support;

use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::{Adapter, Plan, RuntimeSpec, UsageSource};
use apsp_adapter_singbox::SingboxAdapter;
use apsp_compiler::{
    Access, Node, ProtocolConfig, SsMethod, TlsConfig, compile_client, compile_server,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::{path::Path, process::Stdio, time::Duration};
use support::{TempDir, TestOps, TestServices};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
};
use uuid::Uuid;

async fn port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0").await?.local_addr()?.port())
}

async fn wait_port(port: u16) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("client failed to listen")
}

async fn transfer(port: u16) -> Result<()> {
    let echo = TcpListener::bind("127.0.0.1:0").await?;
    let destination = echo.local_addr()?.port();
    let task = tokio::spawn(async move {
        let (mut stream, _) = echo.accept().await?;
        let mut data = [0; 4096];
        stream.read_exact(&mut data).await?;
        stream.write_all(&data).await?;
        Ok::<_, std::io::Error>(())
    });
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
        stream.write_all(&[5, 1, 0]).await?;
        let mut hello = [0; 2];
        stream.read_exact(&mut hello).await?;
        ensure!(hello == [5, 0], "SOCKS greeting failed");
        let mut request = vec![5, 1, 0, 1, 127, 0, 0, 1];
        request.extend(destination.to_be_bytes());
        stream.write_all(&request).await?;
        let mut response = [0; 4];
        stream.read_exact(&mut response).await?;
        ensure!(response[..3] == [5, 0, 0], "SOCKS connection failed");
        let tail = match response[3] {
            1 => 6,
            4 => 18,
            _ => anyhow::bail!("unexpected SOCKS address"),
        };
        stream.read_exact(&mut vec![0; tail]).await?;
        stream.write_all(&[42; 4096]).await?;
        let mut received = [0; 4096];
        stream.read_exact(&mut received).await?;
        ensure!(received == [42; 4096], "payload changed in transit");
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("proxy traffic timed out")
    .and_then(|result| result);
    task.abort();
    result
}

async fn transfer_udp(port: u16) -> Result<()> {
    let echo = UdpSocket::bind("127.0.0.1:0").await?;
    let destination = echo.local_addr()?.port();
    let task = tokio::spawn(async move {
        let mut payload = [0; 2048];
        let (size, peer) = echo.recv_from(&mut payload).await?;
        echo.send_to(&payload[..size], peer).await?;
        Ok::<_, std::io::Error>(())
    });
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut control = TcpStream::connect(("127.0.0.1", port)).await?;
        control.write_all(&[5, 1, 0]).await?;
        let mut greeting = [0; 2];
        control.read_exact(&mut greeting).await?;
        ensure!(greeting == [5, 0], "UDP SOCKS greeting failed");
        control.write_all(&[5, 3, 0, 1, 127, 0, 0, 1, 0, 0]).await?;
        let mut response = [0; 10];
        control.read_exact(&mut response).await?;
        ensure!(response[..4] == [5, 0, 0, 1], "UDP association failed");
        let relay = u16::from_be_bytes([response[8], response[9]]);
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        let mut packet = vec![0, 0, 0, 1, 127, 0, 0, 1];
        packet.extend(destination.to_be_bytes());
        packet.extend([43; 1024]);
        socket.send_to(&packet, ("127.0.0.1", relay)).await?;
        let mut received = [0; 2048];
        let size = socket.recv(&mut received).await?;
        ensure!(
            size == 1034 && received[10..size] == [43; 1024],
            "UDP payload changed"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("UDP proxy traffic timed out")
    .and_then(|result| result);
    task.abort();
    result
}

fn spec(directory: &Path, binary: &Path, node: &Node, stats_port: u16) -> Result<RuntimeSpec> {
    let mut config: Value = serde_json::from_str(&compile_server(std::slice::from_ref(node))?)?;
    let listen = format!("127.0.0.1:{stats_port}");
    config["experimental"]["v2ray_api"]["listen"] = listen.clone().into();
    config["inbounds"][0]["listen"] = "127.0.0.1".into();
    let content = config.to_string();
    std::fs::write(directory.join("config.json"), &content)?;
    Ok(RuntimeSpec {
        revision: 1,
        kernel_version: "1.14.2".into(),
        config_hash: "a".repeat(64),
        binary_path: binary.into(),
        revision_dir: directory.into(),
        stats_listen: listen,
        files: [("config.json".into(), content)].into(),
    })
}

fn start(
    binary: &Path,
    config: &Path,
    directory: &Path,
    name: &str,
) -> Result<tokio::process::Child> {
    let log = std::fs::File::create(directory.join(format!("{name}.log")))?;
    Ok(tokio::process::Command::new(binary)
        .args(["run", "-c"])
        .arg(config)
        .stdout(Stdio::null())
        .stderr(log)
        .kill_on_drop(true)
        .spawn()?)
}

#[tokio::test]
#[ignore = "requires openssl and upstream v1.14.2 with Naive, QUIC, ACME and V2Ray API; set APSP_TEST_SINGBOX"]
async fn all_protocols_authenticate_account_and_revoke_with_real_runtime() -> Result<()> {
    let binary = std::path::PathBuf::from(std::env::var("APSP_TEST_SINGBOX")?);
    let directory = TempDir::new();
    let certificate = directory.0.join("certificate.pem");
    let key = directory.0.join("key.pem");
    let output = std::process::Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
            "-noenc",
            "-days",
            "2",
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
    ensure!(
        output.status.success(),
        "test certificate generation failed"
    );
    let tls = TlsConfig::Manual {
        certificate: std::fs::read_to_string(certificate)?,
        key: std::fs::read_to_string(key)?,
    };
    for protocol in [
        ProtocolConfig::Hysteria2 { tls: tls.clone() },
        ProtocolConfig::Shadowsocks2022 {
            method: SsMethod::Aes128,
            password: STANDARD.encode(Uuid::new_v4().as_bytes()),
        },
        ProtocolConfig::Shadowsocks2022 {
            method: SsMethod::Aes256,
            password: STANDARD.encode(
                [
                    Uuid::new_v4().as_bytes().as_slice(),
                    Uuid::new_v4().as_bytes().as_slice(),
                ]
                .concat(),
            ),
        },
        ProtocolConfig::Tuic { tls: tls.clone() },
        ProtocolConfig::Anytls { tls: tls.clone() },
        ProtocolConfig::Naive { tls: tls.clone() },
        ProtocolConfig::SnellV6 {
            psk: STANDARD.encode(
                [
                    Uuid::new_v4().as_bytes().as_slice(),
                    Uuid::new_v4().as_bytes().as_slice(),
                ]
                .concat(),
            ),
        },
    ] {
        let kind = protocol.kind();
        let mut node = Node {
            enabled: true,
            settings: Default::default(),
            id: 3,
            name: kind.into(),
            port: port().await?,
            public_host: "127.0.0.1".into(),
            sni: if protocol.tls().is_some() {
                "proxy.example.com".into()
            } else {
                String::new()
            },
            private_key: String::new(),
            public_key: String::new(),
            short_id: String::new(),
            users: [1, 2]
                .into_iter()
                .map(|user_id| Access {
                    user_id,
                    uuid: Uuid::new_v4(),
                    credential: STANDARD.encode(vec![user_id as u8; protocol.credential_size()]),
                })
                .collect(),
            protocol_config: protocol,
        };
        let stats_port = port().await?;
        let adapter = SingboxAdapter::new();
        let ops = TestOps {
            version: None,
            ..TestOps::default()
        };
        let prepared = adapter
            .prepare(spec(&directory.0, &binary, &node, stats_port)?, &ops)
            .await?;
        let mut server = start(
            &binary,
            &directory.0.join("config.json"),
            &directory.0,
            "server",
        )?;
        let services = TestServices {
            pid: server.id(),
            ..TestServices::default()
        };
        ensure!(
            adapter.health(&prepared, &services).await?,
            "{kind}: server health failed: {}",
            std::fs::read_to_string(directory.0.join("server.log"))?
        );
        if kind == "anytls" || kind == "hysteria2" {
            let mut invalid = prepared.clone();
            let mut config: Value = serde_json::from_str(&invalid.spec.files["config.json"])?;
            config["inbounds"][0]["tls"]["server_name"] = "wrong.example.com".into();
            invalid
                .spec
                .files
                .insert("config.json".into(), config.to_string());
            ensure!(
                !adapter.health(&invalid, &services).await?,
                "{kind}: invalid certificate name was accepted"
            );
        }
        let mut client_config: Value =
            serde_json::from_str(&compile_client(std::slice::from_ref(&node), 1)?)?;
        let client_port = port().await?;
        client_config["inbounds"][0]["listen_port"] = client_port.into();
        std::fs::write(directory.0.join("client.json"), client_config.to_string())?;
        let mut client = start(
            &binary,
            &directory.0.join("client.json"),
            &directory.0,
            "client",
        )?;
        wait_port(client_port).await?;
        if let Err(error) = transfer(client_port).await {
            anyhow::bail!(
                "{kind}: {error:#}; server: {}; client: {}",
                std::fs::read_to_string(directory.0.join("server.log"))?,
                std::fs::read_to_string(directory.0.join("client.log"))?
            );
        }
        transfer_udp(client_port)
            .await
            .with_context(|| format!("{kind}: UDP transfer"))?;
        let counters = adapter.read_counters(&prepared).await?;
        let active = counters
            .iter()
            .find(|counter| counter.stat_name == "u1_n3")
            .context("missing first counter")?;
        let idle = counters
            .iter()
            .find(|counter| counter.stat_name == "u2_n3")
            .context("missing second counter")?;
        ensure!(
            active.uplink >= 4096 && active.downlink >= 4096,
            "{kind}: missing traffic: {active:?}"
        );
        ensure!(
            idle.uplink == 0 && idle.downlink == 0,
            "{kind}: traffic attributed to another authorization"
        );
        node.users.remove(0);
        let updated = adapter
            .prepare(spec(&directory.0, &binary, &node, stats_port)?, &ops)
            .await?;
        adapter.apply(Plan::Reload, &updated, &services).await?;
        ensure!(
            adapter.health(&updated, &services).await?,
            "{kind}: unhealthy after revocation"
        );
        ensure!(
            transfer(client_port).await.is_err(),
            "{kind}: revoked credential still works"
        );
        client.kill().await?;
        client.wait().await?;
        server.kill().await?;
        server.wait().await?;
        println!(
            "{kind}: TLS/transport health, authentication, per-access counters and revocation passed"
        );
    }
    Ok(())
}

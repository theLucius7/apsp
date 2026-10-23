#![forbid(unsafe_code)]

use anyhow::{Context, Result, ensure};
use apsp_compiler::{
    Access, Node, ProtocolConfig, compile_client,
    external::ExternalOutbound,
    paths::{self, Control, Hop},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    process::Child,
    task::{JoinHandle, JoinSet},
};
use uuid::Uuid;

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Tap {
    port: u16,
    bytes: Arc<AtomicU64>,
    task: JoinHandle<()>,
}
impl Drop for Tap {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0").await?.local_addr()?.port())
}
async fn ready(port: u16) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("fixture listener")
}

fn start(binary: &Path, config: &Value, directory: &Path, name: &str) -> Result<Child> {
    start_with_certificate(binary, config, directory, name, None)
}

fn start_with_certificate(
    binary: &Path,
    config: &Value,
    directory: &Path,
    name: &str,
    certificate: Option<&Path>,
) -> Result<Child> {
    let file = directory.join(format!("{name}.json"));
    std::fs::write(&file, serde_json::to_vec(config)?)?;
    let log = std::fs::File::create(directory.join(format!("{name}.log")))?;
    let mut command = tokio::process::Command::new(binary);
    command
        .args(["run", "-c"])
        .arg(file)
        .stdout(Stdio::null())
        .stderr(log)
        .kill_on_drop(true);
    if let Some(certificate) = certificate {
        // Go's Clash delay handler starts from context.Background(), so its
        // URL test does not inherit the configured certificate store. Limit
        // this fixture's trust override to the spawned runtime process.
        let empty = directory.join("empty-certificate-directory");
        std::fs::create_dir_all(&empty)?;
        command
            .env("SSL_CERT_FILE", certificate)
            .env("SSL_CERT_DIR", empty);
    }
    Ok(command.spawn()?)
}

async fn tap(target: u16, http: bool) -> Result<Tap> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let bytes = Arc::new(AtomicU64::new(0));
    let total = bytes.clone();
    let task = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                accepted=listener.accept()=>{
                    let Ok((mut incoming,_))=accepted else{break;};let total=total.clone();
                    connections.spawn(async move{
                        if http {
                            let mut header=Vec::new();let mut byte=[0u8;1];
                            while !header.ends_with(b"\r\n\r\n") {
                                if header.len()>=8192 || incoming.read_exact(&mut byte).await.is_err(){return;}
                                header.push(byte[0]);
                            }
                            let header=String::from_utf8_lossy(&header);
                            let expected=format!("CONNECT 127.0.0.1:{target} HTTP/1.1");
                            let auth=format!("proxy-authorization: basic {}",STANDARD.encode("fixture:external-secret")).to_ascii_lowercase();
                            if !header.starts_with(&expected) || !header.to_ascii_lowercase().contains(&auth){return;}
                        }
                        let Ok(mut outgoing)=TcpStream::connect(("127.0.0.1",target)).await else{return;};
                        if http && incoming.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.is_err(){return;}
                        let (mut read_in,mut write_in)=incoming.split();let(mut read_out,mut write_out)=outgoing.split();
                        let forward=async{let mut buffer=[0u8;8192];loop{let n=read_in.read(&mut buffer).await?;if n==0{break;}total.fetch_add(n as u64,Ordering::Relaxed);write_out.write_all(&buffer[..n]).await?;}write_out.shutdown().await};
                        let reverse=async{tokio::io::copy(&mut read_out,&mut write_in).await?;write_in.shutdown().await};
                        let _=tokio::try_join!(forward,reverse);
                    });
                },
                _=connections.join_next(),if !connections.is_empty()=>{}
            }
        }
    });
    Ok(Tap { port, bytes, task })
}

fn node(id: i64, port: u16, public_port: u16, users: Vec<Access>) -> Node {
    let secret = x25519_dalek::StaticSecret::from([id as u8; 32]);
    let public = x25519_dalek::PublicKey::from(&secret);
    let settings = apsp_compiler::NodeSettings {
        public_port: Some(public_port),
        ..Default::default()
    };
    Node {
        id,
        name: format!("fixture-{id}"),
        port,
        public_host: "127.0.0.1".into(),
        sni: "www.example.com".into(),
        private_key: URL_SAFE_NO_PAD.encode(secret.as_bytes()),
        public_key: URL_SAFE_NO_PAD.encode(public.as_bytes()),
        short_id: "1234abcd".into(),
        users,
        enabled: true,
        settings,
        protocol_config: ProtocolConfig::VlessReality,
    }
}

async fn isolate(mut config: Value, handshake: u16, egress: Option<&str>) -> Result<Value> {
    if let Some(experimental) = config
        .get_mut("experimental")
        .and_then(Value::as_object_mut)
    {
        experimental.remove("v2ray_api");
        if let Some(api) = experimental.get_mut("clash_api") {
            api["external_controller"] = json!(format!("127.0.0.1:{}", port().await?));
        }
    }
    for inbound in config["inbounds"].as_array_mut().into_iter().flatten() {
        inbound["listen"] = json!("127.0.0.1");
        if inbound.pointer("/tls/reality/handshake").is_some() {
            inbound["tls"]["reality"]["handshake"] =
                json!({"server":"127.0.0.1","server_port":handshake});
        }
    }
    if let Some(egress) = egress {
        for outbound in config["outbounds"].as_array_mut().into_iter().flatten() {
            if outbound["type"] == "direct" {
                outbound["inet4_bind_address"] = json!(egress);
            }
        }
    }
    Ok(config)
}

async fn socks(port: u16, command: u8, target: u16) -> Result<TcpStream> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
    stream.write_all(&[5, 1, 0]).await?;
    let mut hello = [0; 2];
    stream.read_exact(&mut hello).await?;
    ensure!(hello == [5, 0], "SOCKS greeting");
    let mut request = vec![5, command, 0, 1, 127, 0, 0, 1];
    request.extend(target.to_be_bytes());
    stream.write_all(&request).await?;
    Ok(stream)
}

async fn transfer_tcp(client: u16) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let destination = listener.local_addr()?.port();
    let echo = tokio::spawn(async move {
        let (mut stream, peer) = listener.accept().await?;
        ensure!(
            peer.ip().to_string() == "127.0.0.4",
            "traffic bypassed final exit"
        );
        let mut bytes = [0; 4096];
        stream.read_exact(&mut bytes).await?;
        stream.write_all(&bytes).await?;
        Ok::<_, anyhow::Error>(())
    });
    let result = tokio::time::timeout(Duration::from_secs(8), async {
        let mut stream = socks(client, 1, destination).await?;
        let mut header = [0; 4];
        stream.read_exact(&mut header).await?;
        ensure!(header[..3] == [5, 0, 0], "SOCKS connection failed");
        let tail = match header[3] {
            1 => 6,
            4 => 18,
            _ => anyhow::bail!("SOCKS reply address"),
        };
        stream.read_exact(&mut vec![0; tail]).await?;
        stream.write_all(&[42; 4096]).await?;
        let mut bytes = [0; 4096];
        stream.read_exact(&mut bytes).await?;
        ensure!(bytes == [42; 4096], "TCP bytes changed");
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("mixed TCP timeout")
    .and_then(|v| v);
    if result.is_err() {
        echo.abort();
    } else {
        echo.await??;
    }
    result
}

async fn transfer_udp(client: u16) -> Result<()> {
    let echo = UdpSocket::bind("127.0.0.1:0").await?;
    let destination = echo.local_addr()?.port();
    let task = tokio::spawn(async move {
        let mut buffer = [0; 2048];
        let (n, peer) = echo.recv_from(&mut buffer).await?;
        ensure!(
            peer.ip().to_string() == "127.0.0.4",
            "UDP bypassed final exit"
        );
        echo.send_to(&buffer[..n], peer).await?;
        Ok::<_, anyhow::Error>(())
    });
    let result = tokio::time::timeout(Duration::from_secs(8), async {
        let mut control = socks(client, 3, 0).await?;
        let mut response = [0; 10];
        control.read_exact(&mut response).await?;
        ensure!(response[..4] == [5, 0, 0, 1], "UDP association");
        let relay = u16::from_be_bytes([response[8], response[9]]);
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        let mut packet = vec![0, 0, 0, 1, 127, 0, 0, 1];
        packet.extend(destination.to_be_bytes());
        packet.extend([43; 1024]);
        socket.send_to(&packet, ("127.0.0.1", relay)).await?;
        let mut bytes = [0; 2048];
        let n = socket.recv(&mut bytes).await?;
        ensure!(n == 1034 && bytes[10..n] == [43; 1024], "UDP bytes changed");
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("mixed UDP timeout")
    .and_then(|v| v);
    if result.is_err() {
        task.abort();
    } else {
        task.await??;
    }
    result
}

async fn tls_fixture(directory: &Path) -> Result<(u16, PathBuf, Child)> {
    let certificate = directory.join("certificate.pem");
    let key = directory.join("key.pem");
    ensure!(
        tokio::process::Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-days",
                "1",
                "-subj",
                "/CN=www.example.com",
                "-addext",
                "subjectAltName=DNS:www.example.com,IP:127.0.0.1",
                "-addext",
                "basicConstraints=critical,CA:FALSE",
                "-keyout"
            ])
            .arg(&key)
            .arg("-out")
            .arg(&certificate)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?
            .success(),
        "certificate fixture"
    );
    let handshake = port().await?;
    let tls = tokio::process::Command::new("openssl")
        .args(["s_server", "-accept"])
        .arg(format!("127.0.0.1:{handshake}"))
        .arg("-cert")
        .arg(&certificate)
        .arg("-key")
        .arg(key)
        .args(["-www", "-tls1_3"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    ready(handshake).await?;
    Ok((handshake, certificate, tls))
}

async fn https_target(directory: &Path, certificate: &Path) -> Result<(u16, Child)> {
    let target = port().await?;
    let log = std::fs::File::create(directory.join("https-target.log"))?;
    // The native delay API sends HEAD. OpenSSL's diagnostic -www mode does not
    // provide a reliable HEAD response, so use a bounded local HTTP handler.
    let script = r#"
import http.server, ssl, sys, time
class Handler(http.server.BaseHTTPRequestHandler):
    def do_HEAD(self):
        time.sleep(0.025)
        print(self.client_address[0], self.command, self.path, flush=True)
        self.send_response(204)
        self.send_header('Content-Length', '0')
        self.end_headers()
    def log_message(self, *_):
        pass
server = http.server.ThreadingHTTPServer(('127.0.0.1', int(sys.argv[3])), Handler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.minimum_version = ssl.TLSVersion.TLSv1_3
context.load_cert_chain(sys.argv[1], sys.argv[2])
server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()
"#;
    let child = tokio::process::Command::new("python3")
        .args(["-u", "-c", script])
        .arg(certificate)
        .arg(directory.join("key.pem"))
        .arg(target.to_string())
        .stdout(log)
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    ready(target).await?;
    Ok((target, child))
}

#[tokio::test]
#[ignore = "requires APSP_TEST_UPSTREAM=official v1.14.2, OpenSSL and isolated loopback ports; excludes unavailable official accounting extension"]
async fn external_middle_hop_carries_tcp_udp_and_never_bypasses_a_stopped_hop() -> Result<()> {
    let binary = PathBuf::from(std::env::var("APSP_TEST_UPSTREAM")?);
    let directory =
        Directory(std::env::temp_dir().join(format!("apsp-mixed-runtime-{}", Uuid::new_v4())));
    std::fs::create_dir_all(&directory.0)?;
    let (handshake, _, mut tls) = tls_fixture(&directory.0).await?;
    for with_middle in [false, true] {
        let a_port = port().await?;
        let m_port = port().await?;
        let b_port = port().await?;
        let a_tap = tap(a_port, false).await?;
        let m_tap = tap(m_port, false).await?;
        let b_tap = tap(b_port, false).await?;
        let external = tap(b_tap.port, true).await?;
        let access = Access {
            user_id: 7,
            uuid: Uuid::from_u128(7),
            credential: String::new(),
        };
        let entry = node(1, a_port, a_tap.port, vec![access]);
        let middle = node(2, m_port, m_tap.port, vec![]);
        let exit = node(3, b_port, b_tap.port, vec![]);
        let mut hops = Vec::new();
        if with_middle {
            hops.push(Hop::Managed {
                server_id: 2,
                endpoint: Box::new(middle.clone()),
                identity: Uuid::from_u128(1002),
            });
        }
        hops.push(Hop::External{node_id:10,version_id:10,outbound:ExternalOutbound(json!({"type":"http","server":"127.0.0.1","server_port":external.port,"username":"fixture","password":"external-secret"}))});
        hops.push(Hop::Managed {
            server_id: 3,
            endpoint: Box::new(exit.clone()),
            identity: Uuid::from_u128(1003),
        });
        let model = paths::Path {
            chain_id: 1,
            generation: 1,
            entry_server_id: 1,
            entry_node_id: 1,
            active: true,
            hops,
        };
        ensure!(
            paths::validate(&model)?.udp,
            "HTTP middle must carry the final Reality TCP transport for UDP"
        );
        let control = Control {
            secret: "fixture-control-secret-01234567890123456789".into(),
            test_url: "https://probe.example.com/ready".into(),
        };
        let mut processes = Vec::new();
        for (server, endpoint, egress) in [
            (3, exit, Some("127.0.0.4")),
            (2, middle, None),
            (1, entry.clone(), None),
        ] {
            if server == 2 && !with_middle {
                continue;
            }
            let compiled = paths::compile(
                server,
                std::slice::from_ref(&endpoint),
                &[],
                std::slice::from_ref(&model),
                &[],
                BTreeMap::new(),
                (server == 1).then_some(&control),
            )?;
            let config =
                isolate(serde_json::from_str(&compiled.config)?, handshake, egress).await?;
            processes.push(start(
                &binary,
                &config,
                &directory.0,
                &format!("case-{with_middle}-server-{server}"),
            )?);
            ready(endpoint.port).await?;
        }
        let client_port = port().await?;
        let mut client: Value = serde_json::from_str(&compile_client(&[entry], 7)?)?;
        client["inbounds"][0]["listen_port"] = json!(client_port);
        processes.push(start(
            &binary,
            &client,
            &directory.0,
            &format!("case-{with_middle}-client"),
        )?);
        ready(client_port).await?;
        let traffic = async {
            transfer_tcp(client_port).await?;
            transfer_udp(client_port).await?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if let Err(error) = traffic {
            for file in std::fs::read_dir(&directory.0)? {
                let file = file?.path();
                if file.extension().is_some_and(|v| v == "log") {
                    eprintln!("{}\n{}", file.display(), std::fs::read_to_string(&file)?);
                }
            }
            return Err(error);
        }
        ensure!(
            external.bytes.load(Ordering::Relaxed) > 4096
                && b_tap.bytes.load(Ordering::Relaxed) > 4096
                && a_tap.bytes.load(Ordering::Relaxed) > 4096,
            "a required hop was not observed"
        );
        if with_middle {
            ensure!(
                m_tap.bytes.load(Ordering::Relaxed) > 4096,
                "managed middle not traversed"
            );
        }
        drop(external);
        ensure!(
            transfer_tcp(client_port).await.is_err(),
            "TCP bypassed the stopped subscription hop"
        );
        ensure!(
            transfer_udp(client_port).await.is_err(),
            "UDP bypassed the stopped subscription hop"
        );
        for process in &mut processes {
            let _ = process.kill().await;
            let _ = process.wait().await;
        }
    }
    let _ = tls.kill().await;
    let _ = tls.wait().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires official v1.14.2, OpenSSL, Python and exclusive loopback port 18086; proves native Clash path validation, not distributed Agent acceptance"]
async fn clash_delay_checks_the_compiled_path_and_fails_when_external_middle_stops() -> Result<()> {
    let binary = PathBuf::from(std::env::var("APSP_TEST_UPSTREAM")?);
    let directory =
        Directory(std::env::temp_dir().join(format!("apsp-mixed-clash-{}", Uuid::new_v4())));
    std::fs::create_dir_all(&directory.0)?;
    let (handshake, certificate, mut tls) = tls_fixture(&directory.0).await?;
    let (target, mut target_process) = https_target(&directory.0, &certificate).await?;
    let controller = TcpListener::bind(("127.0.0.1", paths::CONTROL_PORT))
        .await
        .context("exclusive native controller fixture port")?;
    let a_port = port().await?;
    let b_port = port().await?;
    let b_tap = tap(b_port, false).await?;
    let external = tap(b_tap.port, true).await?;
    let entry = node(1, a_port, a_port, vec![]);
    let exit = node(3, b_port, b_tap.port, vec![]);
    let model = paths::Path {
        chain_id: 91,
        generation: 2,
        entry_server_id: 1,
        entry_node_id: 1,
        active: true,
        hops: vec![
            Hop::External {
                node_id: 10,
                version_id: 10,
                outbound: ExternalOutbound(
                    json!({"type":"http","server":"127.0.0.1","server_port":external.port,"username":"fixture","password":"external-secret"}),
                ),
            },
            Hop::Managed {
                server_id: 3,
                endpoint: Box::new(exit.clone()),
                identity: Uuid::from_u128(9103),
            },
        ],
    };
    let control = Control {
        secret: "fixture-control-secret-01234567890123456789".into(),
        test_url: format!("https://127.0.0.1:{target}/ready"),
    };
    let compiled_exit = paths::compile(
        3,
        &[exit],
        &[],
        std::slice::from_ref(&model),
        &[],
        BTreeMap::new(),
        None,
    )?;
    let config_exit = isolate(
        serde_json::from_str(&compiled_exit.config)?,
        handshake,
        Some("127.0.0.4"),
    )
    .await?;
    let mut exit_process = start(&binary, &config_exit, &directory.0, "exit")?;
    ready(b_port).await?;
    let compiled_entry = paths::compile(
        1,
        &[entry],
        &[],
        &[model],
        &[],
        BTreeMap::new(),
        Some(&control),
    )?;
    let check = compiled_entry
        .checks
        .first()
        .context("compiled path check")?;
    let mut config: Value = serde_json::from_str(&compiled_entry.config)?;
    let controller_config = config["experimental"]["clash_api"].clone();
    ensure!(
        controller_config["external_controller"] == "127.0.0.1:18086"
            && controller_config["secret"] == control.secret
    );
    config = isolate(config, handshake, None).await?;
    config["experimental"]["clash_api"] = controller_config;
    config["certificate"] = json!({"store":"none","certificate_path":[certificate]});
    drop(controller);
    let mut entry_process =
        start_with_certificate(&binary, &config, &directory.0, "entry", Some(&certificate))?;
    ready(paths::CONTROL_PORT).await?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(12))
        .build()?;
    let version: Value = client
        .get(format!("http://127.0.0.1:{}/version", paths::CONTROL_PORT))
        .bearer_auth(&control.secret)
        .send()
        .await?
        .json()
        .await?;
    ensure!(
        version["version"] == "sing-box 1.14.2",
        "fixture requires the fixed official runtime version"
    );
    let mut url = reqwest::Url::parse(&format!(
        "http://127.0.0.1:{}/proxies/{}/delay",
        paths::CONTROL_PORT,
        check.outbound
    ))?;
    url.query_pairs_mut()
        .append_pair("url", &check.url)
        .append_pair("timeout", "10000");
    let unauthorized = client.get(url.clone()).send().await?;
    ensure!(
        unauthorized.status() == reqwest::StatusCode::UNAUTHORIZED,
        "native controller accepted missing authentication"
    );
    ensure!(
        external.bytes.load(Ordering::Relaxed) == 0,
        "unauthorized request opened the path"
    );
    let response = client
        .get(url.clone())
        .bearer_auth(&control.secret)
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if status != reqwest::StatusCode::OK {
        eprintln!(
            "entry log: {}",
            std::fs::read_to_string(directory.0.join("entry.log"))?
        );
        eprintln!(
            "exit log: {}",
            std::fs::read_to_string(directory.0.join("exit.log"))?
        );
    }
    ensure!(
        status == reqwest::StatusCode::OK && body["delay"].as_u64().is_some_and(|delay| delay > 0),
        "native path delay failed: {status} {body}"
    );
    ensure!(
        external.bytes.load(Ordering::Relaxed) > 0 && b_tap.bytes.load(Ordering::Relaxed) > 0,
        "native path check skipped a required hop"
    );
    ensure!(
        std::fs::read_to_string(directory.0.join("https-target.log"))?.trim()
            == "127.0.0.4 HEAD /ready",
        "native HTTPS check did not arrive from the selected final exit"
    );
    drop(external);
    let response = client.get(url).bearer_auth(&control.secret).send().await?;
    ensure!(
        [
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            reqwest::StatusCode::GATEWAY_TIMEOUT
        ]
        .contains(&response.status()),
        "native delay check bypassed stopped subscription hop"
    );
    let direct = reqwest::Client::builder()
        .no_proxy()
        .add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(
            &certificate,
        )?)?)
        .build()?;
    ensure!(
        direct.head(&control.test_url).send().await?.status() == reqwest::StatusCode::NO_CONTENT,
        "target stopped along with the external hop"
    );
    for process in [
        &mut entry_process,
        &mut exit_process,
        &mut tls,
        &mut target_process,
    ] {
        let _ = process.kill().await;
        let _ = process.wait().await;
    }
    Ok(())
}

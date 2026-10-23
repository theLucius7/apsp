//! Private loopback Reality fixture for a subscription imported before migration.
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::{Counter, Prepared, RuntimeSpec, UsageSource};
use apsp_adapter_singbox::SingboxAdapter;
use apsp_compiler::Node;
use serde_json::Value;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::{Child, Command},
};

// The single-node subscription contract from compiler at 6a583af, before
// the plugin business migration. Do not regenerate this from today's compiler.
pub fn legacy_client(node: &Node, user_id: i64) -> Value {
    let access = node
        .users
        .iter()
        .find(|access| access.user_id == user_id)
        .expect("imported grant");
    let tag = format!("node-{}", node.id);
    serde_json::json!({
        "log": {"level":"warn","timestamp":true},
        "inbounds":[{"type":"mixed","tag":"mixed-in","listen":"127.0.0.1","listen_port":2080}],
        "outbounds":[
            {"type":"selector","tag":"proxy","outbounds":[tag]},
            {"type":"vless","tag":tag,"server":node.public_host,"server_port":node.port,
             "uuid":access.uuid,"flow":"xtls-rprx-vision",
             "tls":{"enabled":true,"server_name":node.sni,"utls":{"enabled":true,"fingerprint":"chrome"},
                    "reality":{"enabled":true,"public_key":node.public_key,"short_id":node.short_id}}},
            {"type":"direct","tag":"direct"}
        ],
        "route":{"final":"proxy"}
    })
}

pub async fn port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0").await?.local_addr()?.port())
}

pub struct Runtime {
    directory: PathBuf,
    children: Vec<Child>,
    prepared: Prepared,
    client_port: u16,
    pids: Vec<u32>,
    cached_client: Value,
}

impl Runtime {
    pub async fn start(binary: PathBuf, node: &Node, client: &Value) -> Result<Self> {
        let version = tokio::time::timeout(
            Duration::from_secs(5),
            Command::new(&binary)
                .arg("version")
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        let version_text = String::from_utf8(version.stdout)?;
        ensure!(
            version.status.success()
                && version_text.lines().next() == Some("sing-box version 1.14.2")
                && version_text.contains("with_v2ray_api"),
            "requires pinned statistics-enabled sing-box 1.14.2"
        );
        let directory =
            std::env::temp_dir().join(format!("apsp-imported-runtime-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        let tls_port = port().await?;
        let stats_port = port().await?;
        let client_port = port().await?;
        let mut config: Value =
            serde_json::from_str(&apsp_compiler::compile_server(std::slice::from_ref(node))?)?;
        // Only transport endpoints change: credentials and authorization remain compiled.
        config["inbounds"][0]["listen"] = "127.0.0.1".into();
        config["inbounds"][0]["tls"]["reality"]["handshake"]["server"] = "127.0.0.1".into();
        config["inbounds"][0]["tls"]["reality"]["handshake"]["server_port"] = tls_port.into();
        let stats_listen = format!("127.0.0.1:{stats_port}");
        config["experimental"]["v2ray_api"]["listen"] = stats_listen.clone().into();
        let config = serde_json::to_string(&config)?;
        let mut runtime = Self {
            prepared: Prepared {
                spec: RuntimeSpec {
                    revision: 7,
                    kernel_version: "1.14.2".into(),
                    config_hash: "fixture".into(),
                    binary_path: binary.clone(),
                    revision_dir: directory.clone(),
                    stats_listen,
                    files: [("config.json".into(), config.clone())].into(),
                },
                listen_ports: vec![node.port],
            },
            directory,
            children: Vec::new(),
            client_port,
            pids: Vec::new(),
            cached_client: client.clone(),
        };
        let certificate = runtime.directory.join("tls.crt");
        let key = runtime.directory.join("tls.key");
        let generated = tokio::time::timeout(
            Duration::from_secs(10),
            Command::new("openssl")
                .args([
                    "req",
                    "-x509",
                    "-newkey",
                    "ec",
                    "-pkeyopt",
                    "ec_paramgen_curve:P-256",
                    "-nodes",
                    "-days",
                    "1",
                    "-subj",
                    "/CN=www.example.com",
                ])
                .arg("-keyout")
                .arg(&key)
                .arg("-out")
                .arg(&certificate)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status(),
        )
        .await??;
        ensure!(generated.success(), "fixture certificate generation failed");
        let mut tls = Command::new("openssl");
        tls.args([
            "s_server", "-quiet", "-www", "-tls1_3", "-groups", "X25519", "-accept",
        ])
        .arg(format!("127.0.0.1:{tls_port}"))
        .arg("-cert")
        .arg(certificate)
        .arg("-key")
        .arg(key);
        runtime.spawn(tls, "tls")?;
        Self::ready(tls_port).await?;
        let server_path = runtime.directory.join("server.json");
        std::fs::write(&server_path, config)?;
        let mut server = Command::new(&binary);
        server.arg("run").arg("-c").arg(server_path);
        runtime.spawn(server, "server")?;
        Self::ready(node.port).await?;
        let mut client_config = client.clone();
        client_config["inbounds"][0]["listen_port"] = client_port.into();
        let client_path = runtime.directory.join("client.json");
        std::fs::write(&client_path, serde_json::to_vec(&client_config)?)?;
        let mut command = Command::new(binary);
        command.arg("run").arg("-c").arg(client_path);
        runtime.spawn(command, "client")?;
        Self::ready(client_port).await?;
        runtime.pids = runtime
            .children
            .iter()
            .map(|child| child.id().expect("live child"))
            .collect();
        Ok(runtime)
    }

    fn spawn(&mut self, mut command: Command, label: &str) -> Result<()> {
        let log = std::fs::File::create(self.directory.join(format!("{label}.log")))?;
        let child = command
            .stdout(log.try_clone()?)
            .stderr(log)
            .kill_on_drop(true)
            .spawn()?;
        self.children.push(child);
        Ok(())
    }

    async fn ready(port: u16) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .context("private runtime listener did not become ready")
    }

    pub async fn traffic(&mut self) -> Result<Counter> {
        ensure!(
            self.children
                .iter_mut()
                .all(|child| child.try_wait().is_ok_and(|exit| exit.is_none())),
            "fixture process exited"
        );
        ensure!(
            self.children
                .iter()
                .filter_map(Child::id)
                .collect::<Vec<_>>()
                == self.pids,
            "migration restarted runtime or cached client"
        );
        let echo = TcpListener::bind("127.0.0.1:0").await?;
        let echo_port = echo.local_addr()?.port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = echo.accept().await?;
            let mut payload = [0_u8; 8192];
            stream.read_exact(&mut payload).await?;
            stream.write_all(&payload).await?;
            stream.shutdown().await?;
            Ok::<_, std::io::Error>(())
        });
        let result = tokio::time::timeout(Duration::from_secs(8), async {
            let mut stream = TcpStream::connect(("127.0.0.1", self.client_port)).await?;
            stream.write_all(&[5, 1, 0]).await?;
            let mut method = [0; 2];
            stream.read_exact(&mut method).await?;
            ensure!(method == [5, 0], "SOCKS negotiation failed");
            let mut request = vec![5, 1, 0, 1, 127, 0, 0, 1];
            request.extend_from_slice(&echo_port.to_be_bytes());
            stream.write_all(&request).await?;
            let mut response = [0; 10];
            stream.read_exact(&mut response).await?;
            ensure!(response[..4] == [5, 0, 0, 1], "SOCKS connection rejected");
            stream.write_all(&[42; 8192]).await?;
            let mut response = [0; 8192];
            stream.read_exact(&mut response).await?;
            ensure!(response == [42; 8192], "Reality echo payload differs");
            stream.shutdown().await?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("cached subscription traffic timed out");
        server.abort();
        result??;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let counters = SingboxAdapter::new().read_counters(&self.prepared).await?;
        ensure!(
            counters.len() == 1 && counters[0].uplink >= 8192 && counters[0].downlink >= 8192,
            "runtime did not attribute traffic to imported authorization"
        );
        Ok(counters[0].clone())
    }

    pub fn cached_client(&self) -> &Value {
        &self.cached_client
    }

    pub async fn stop(mut self) -> Result<()> {
        for child in self.children.iter_mut().rev() {
            child.kill().await?;
            child.wait().await?;
        }
        Ok(())
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        for child in self.children.iter_mut().rev() {
            let _ = child.start_kill();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

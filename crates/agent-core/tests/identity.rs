#![forbid(unsafe_code)]
#![cfg(unix)]

use anyhow::{Context, Result};
use apsp_agent_core::{Config, config::validate_panel_url, identity};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};
use uuid::Uuid;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("apsp-identity-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn config(&self, origin: &str) -> Config {
        Config {
            settings: apsp_protocol::AgentSettings::default(),
            panel_url: origin.into(),
            panel_ca_file: None,
            identity_dir: self.0.join("identity"),
            state_db: self.0.join("state.db"),
            runtime_root: self.0.join("runtime"),
            install_root: self.0.join("install"),
            agent_root: self.0.join("core"),
            status_socket: self.0.join("status.sock"),
            operation_timeout_secs: 5,
            public_ips: vec![],
            allow_remote_commands: false,
        }
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Request {
    body: Value,
    reply: oneshot::Sender<String>,
}
struct MockPanel {
    origin: String,
    requests: mpsc::Receiver<Request>,
    task: JoinHandle<()>,
}
impl MockPanel {
    async fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let (sender, requests) = mpsc::channel(8);
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let body = read_request(&mut stream).await.unwrap();
                let (reply, response) = oneshot::channel();
                if sender.send(Request { body, reply }).await.is_err() {
                    break;
                }
                if let Ok(response) = response.await {
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            }
        });
        Ok(Self {
            origin,
            requests,
            task,
        })
    }
    async fn next(&mut self) -> Result<Request> {
        timeout(Duration::from_secs(5), self.requests.recv())
            .await?
            .context("mock panel ended")
    }
}
impl Drop for MockPanel {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_request(stream: &mut TcpStream) -> Result<Value> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut buffer).await?;
        anyhow::ensure!(count > 0, "request ended before headers");
        bytes.extend_from_slice(&buffer[..count]);
        anyhow::ensure!(bytes.len() < 65536, "request too large");
        if let Some(position) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end])?;
    assert!(headers.starts_with("POST /api/agent/v1/enroll HTTP/1.1\r\n"));
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())
                .flatten()
        })
        .context("request content length")?;
    while bytes.len() < header_end + length {
        let count = stream.read(&mut buffer).await?;
        anyhow::ensure!(count > 0, "request body truncated");
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(serde_json::from_slice(
        &bytes[header_end..header_end + length],
    )?)
}

fn response(status: &str, body: &Value) -> String {
    let body = body.to_string();
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn enrollment_persists_key_before_network_and_retries_keep_identity() -> Result<()> {
    let directory = Directory::new();
    let mut panel = MockPanel::start().await?;
    let config = directory.config(&panel.origin);
    let first_config = config.clone();
    let first = tokio::spawn(async move { identity::enroll(&first_config, "test-token").await });
    let request = panel.next().await?;
    let key_path = config.identity_dir.join("device.key");
    let original = std::fs::read(&key_path)?;
    assert_eq!(original.len(), 32);
    assert_eq!(request.body["token"], "test-token");
    let public_key = request.body["device_public_key"]
        .as_str()
        .context("public key")?
        .to_owned();
    assert_eq!(URL_SAFE_NO_PAD.decode(&public_key)?.len(), 32);
    assert!(!config.identity_dir.join("server_id").exists());
    request
        .reply
        .send(response("503 Service Unavailable", &json!({})))
        .unwrap();
    assert!(first.await?.is_err());
    assert_eq!(std::fs::read(&key_path)?, original);

    let next_config = config.clone();
    let retry =
        tokio::spawn(async move { identity::enroll(&next_config, "replacement-token").await });
    let request = panel.next().await?;
    assert_eq!(request.body["device_public_key"], public_key);
    request
        .reply
        .send(response("200 OK", &json!({"server_id": 7})))
        .unwrap();
    assert_eq!(retry.await??, 7);
    let identity = identity::load(&config)?;
    assert_eq!(identity.server_id, 7);
    assert_eq!(identity.signing_key.to_bytes().as_slice(), original);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&key_path)?.permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&config.identity_dir)?
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    let mut other_panel = config.clone();
    other_panel.panel_url = "https://other-panel.example.test".into();
    assert!(identity::load(&other_panel).is_err());
    assert!(identity::enroll(&other_panel, "test-token").await.is_err());
    assert_eq!(std::fs::read(key_path)?, original);
    Ok(())
}

#[tokio::test]
async fn enrollment_reports_status_and_direction_without_echoing_untrusted_bodies() -> Result<()> {
    let directory = Directory::new();
    let mut panel = MockPanel::start().await?;
    let config = directory.config(&panel.origin);
    let token = "private-enrollment-token";
    for (status, guidance) in [
        ("401 Unauthorized", "token validity"),
        ("403 Forbidden", "CDN/WAF"),
        ("429 Too Many Requests", "rate limits"),
        ("503 Service Unavailable", "panel health"),
    ] {
        let next_config = config.clone();
        let task = tokio::spawn(async move { identity::enroll(&next_config, token).await });
        let request = panel.next().await?;
        request
            .reply
            .send(response(status, &json!({"error": token})))
            .unwrap();
        let error = task.await?.expect_err("a rejected request cannot enroll");
        let message = format!("{error:#}");
        assert!(message.contains(status), "{message}");
        assert!(message.contains(guidance), "{message}");
        assert!(!message.contains(token));
        assert!(!config.identity_dir.join("server_id").exists());
    }
    Ok(())
}

#[tokio::test]
async fn enrollment_refuses_redirects_without_contacting_another_origin() -> Result<()> {
    let directory = Directory::new();
    let mut panel = MockPanel::start().await?;
    let outside = TcpListener::bind("127.0.0.1:0").await?;
    let config = directory.config(&panel.origin);
    let task = tokio::spawn(async move { identity::enroll(&config, "test-token").await });
    let request = panel.next().await?;
    request.reply.send(format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{}/must-not-receive-token\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", outside.local_addr()?)).unwrap();
    assert!(task.await?.is_err());
    assert!(
        timeout(Duration::from_millis(100), outside.accept())
            .await
            .is_err()
    );
    Ok(())
}

#[test]
fn minimal_configuration_has_defaults_and_rejects_ambiguous_panel_origins() -> Result<()> {
    let directory = Directory::new();
    let path = directory.0.join("agent.toml");
    std::fs::write(&path, "panel_url = 'https://panel.example.test'\n")?;
    let config = Config::load(&path)?;
    assert_eq!(config.operation_timeout_secs, 30);
    let identity_dir = if cfg!(target_os = "macos") {
        "/private/etc/apsp/identity"
    } else {
        "/etc/apsp/identity"
    };
    assert_eq!(config.identity_dir, PathBuf::from(identity_dir));
    for invalid in [
        "file:///tmp/panel",
        "https://account:secret@panel.example.test",
        "https://panel.example.test/path",
        "https://panel.example.test?token=x",
        "https://panel.example.test#fragment",
    ] {
        assert!(validate_panel_url(invalid).is_err(), "{invalid}");
    }
    let mut config = config;
    config.state_db = "relative.db".into();
    assert!(config.validate().is_err());
    Ok(())
}

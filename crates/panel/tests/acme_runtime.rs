#![forbid(unsafe_code)]

#[path = "../../adapter-singbox/tests/support/mod.rs"]
#[allow(dead_code)]
mod support;

use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::{Adapter, Plan, RuntimeSpec};
use apsp_adapter_singbox::SingboxAdapter;
use apsp_compiler::{Access, AcmeChallenge, Node, ProtocolConfig, TlsConfig, compile_server};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use support::{TempDir, TestOps, TestServices};
use tokio::net::TcpListener;
use uuid::Uuid;

async fn port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0").await?.local_addr()?.port())
}

fn certificates(directory: &Path) -> Result<Vec<String>> {
    let mut contents = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            contents.extend(certificates(&entry.path())?);
        } else if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "crt")
        {
            contents.push(std::fs::read_to_string(entry.path())?);
        }
    }
    contents.sort();
    Ok(contents)
}

#[tokio::test]
#[ignore = "requires local Pebble and fixed runtime; run tools/acme-smoke.py"]
async fn actual_challenges_persistence_and_expired_certificate_renewal() -> Result<()> {
    let binary = PathBuf::from(std::env::var("APSP_TEST_SINGBOX")?);
    let provider = std::env::var("APSP_TEST_ACME_URL")?;
    ensure!(
        provider.starts_with("https://127.0.0.1:"),
        "test CA must be loopback"
    );
    let bootstrap = std::fs::read_to_string(std::env::var("APSP_TEST_ACME_CA")?)?;
    let issuer = std::fs::read_to_string(std::env::var("APSP_TEST_ACME_ISSUER")?)?;
    for challenge in [AcmeChallenge::Http01, AcmeChallenge::TlsAlpn01] {
        let directory = TempDir::new();
        let data = directory.0.join("data");
        std::fs::create_dir(&data)?;
        let node = Node {
            enabled: true,
            settings: Default::default(),
            id: 1,
            name: "ACME test".into(),
            port: port().await?,
            public_host: "127.0.0.1".into(),
            sni: "proxy.example.com".into(),
            private_key: String::new(),
            public_key: String::new(),
            short_id: String::new(),
            users: vec![Access {
                user_id: 1,
                uuid: Uuid::new_v4(),
                credential: STANDARD.encode([1; 32]),
            }],
            protocol_config: ProtocolConfig::Anytls {
                tls: TlsConfig::Acme {
                    email: "test@example.com".into(),
                    challenge,
                },
            },
        };
        let tls = node.protocol_config.tls().unwrap().clone();
        let mut nodes = vec![node];
        for protocol in [
            ProtocolConfig::Hysteria2 { tls: tls.clone() },
            ProtocolConfig::Tuic { tls: tls.clone() },
            ProtocolConfig::Naive { tls },
        ] {
            let mut node = nodes[0].clone();
            node.id = nodes.len() as i64 + 1;
            node.port = port().await?;
            node.protocol_config = protocol;
            nodes.push(node);
        }
        let mut config: Value = serde_json::from_str(&compile_server(&nodes)?)?;
        let listen = format!("127.0.0.1:{}", port().await?);
        config["experimental"]["v2ray_api"]["listen"] = listen.clone().into();
        for inbound in config["inbounds"].as_array_mut().unwrap() {
            inbound["listen"] = "127.0.0.1".into();
            inbound["tls"]["certificate"] = issuer.clone().into();
        }
        // Only this fixture trusts the temporary test CA. Production config uses public roots.
        config["certificate"] = json!({"certificate": bootstrap});
        config["certificate_providers"][0]["provider"] = provider.clone().into();
        config["certificate_providers"][0]["alternative_http_port"] =
            std::env::var("APSP_TEST_ACME_HTTP_PORT")?
                .parse::<u16>()?
                .into();
        config["certificate_providers"][0]["alternative_tls_port"] =
            std::env::var("APSP_TEST_ACME_TLS_PORT")?
                .parse::<u16>()?
                .into();
        let content = config.to_string();
        let path = directory.0.join("config.json");
        std::fs::write(&path, &content)?;
        let adapter = SingboxAdapter::new();
        let prepared = adapter
            .prepare(
                RuntimeSpec {
                    revision: 1,
                    kernel_version: "1.14.2".into(),
                    config_hash: "a".repeat(64),
                    binary_path: binary.clone(),
                    revision_dir: directory.0.clone(),
                    stats_listen: listen,
                    files: [("config.json".into(), content)].into(),
                },
                &TestOps {
                    version: None,
                    ..TestOps::default()
                },
            )
            .await?;
        ensure!(
            adapter.health_timeout(&prepared) == Duration::from_secs(245),
            "missing bounded issuance budget"
        );
        let log = directory.0.join("runtime.log");
        let mut child = tokio::process::Command::new(&binary)
            .args(["run", "-c"])
            .arg(&path)
            .arg("-D")
            .arg(&data)
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log)?)
            .kill_on_drop(true)
            .spawn()?;
        let services = TestServices {
            pid: child.id(),
            ..TestServices::default()
        };
        let ready = tokio::time::timeout(
            Duration::from_secs(40),
            adapter.health(&prepared, &services),
        )
        .await;
        ensure!(
            matches!(ready, Ok(Ok(true))),
            "{challenge:?}: issuance failed: {}",
            std::fs::read_to_string(&log)?
        );
        let issued = certificates(&data.join("certificates"))?;
        ensure!(
            issued.len() == 1,
            "certificate not stored in persistent data directory"
        );
        adapter.apply(Plan::Reload, &prepared, &services).await?;
        ensure!(
            adapter.health(&prepared, &services).await?,
            "certificate not reused after reload"
        );
        ensure!(
            certificates(&data.join("certificates"))? == issued,
            "reload unnecessarily reissued certificate"
        );
        println!("{challenge:?}: real challenge, trusted handshake and persistent reuse passed");
        if challenge == AcmeChallenge::Http01 {
            // Pebble issues 60-second certificates; an expired cache must renew on restart.
            tokio::time::sleep(Duration::from_secs(61)).await;
            adapter.apply(Plan::Reload, &prepared, &services).await?;
            let renewed = tokio::time::timeout(
                Duration::from_secs(40),
                adapter.health(&prepared, &services),
            )
            .await
            .context("renewal deadline")??;
            ensure!(
                renewed,
                "renewed certificate not usable: {}",
                std::fs::read_to_string(&log)?
            );
            ensure!(
                certificates(&data.join("certificates"))? != issued,
                "expired certificate was not renewed"
            );
            println!("Http01: expired cached certificate automatically renewed after reload");
        }
        child.kill().await?;
        child.wait().await?;
    }
    Ok(())
}

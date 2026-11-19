#![forbid(unsafe_code)]
mod business_support;
#[path = "probe_support.rs"]
mod probe_support;
use business_support::release_fixture;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_observability::diagnostic_tools::tcpquality::TOOL_VERSION;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

async fn ready(panel: &TestPanel, server: i64) -> Result<()> {
    use apsp_protocol::{
        bundled::{
            EmbeddedAgentManifest, EmbeddedComponentRecord, encode_embedded_agent_manifest_frame,
        },
        release::{ReleaseMetadata, canonical_asset_name},
    };
    use std::collections::BTreeMap;

    let installer = b"#!/bin/sh\n# APSP_BOOTSTRAP_AGENT_SOURCE=preloaded-github-v1\nexit 0\n";
    let binary = b"TEST_ONLY panel routing fixture; never executed";
    let archive = release_fixture::archive("nodequality", binary)?;
    let nodequality_sha = release_support::hash(&archive);
    let node_ver = apsp_observability::diagnostics::TOOL_VERSION;

    let mut embedded = BTreeMap::new();
    embedded.insert(
        "nodequality".into(),
        EmbeddedComponentRecord {
            format: "tar.gz".into(),
            inventory_sha256: None,
            sha256: Some(nodequality_sha.clone()),
            subcommand: None,
            version: node_ver.into(),
        },
    );
    embedded.insert(
        "tcpquality".into(),
        EmbeddedComponentRecord {
            format: "agent_subcommand".into(),
            inventory_sha256: None,
            sha256: None,
            subcommand: Some("tcp-probe".into()),
            version: TOOL_VERSION.into(),
        },
    );

    let mut artifacts = Vec::new();
    let mut amd64_sha = String::new();
    for (target, rust_arch) in [
        ("linux-musl-amd64", "x86_64"),
        ("linux-musl-arm64", "aarch64"),
    ] {
        let manifest = EmbeddedAgentManifest {
            schema: 1,
            agent_version: "0.4.0".into(),
            target_os: "linux".into(),
            target_arch: rust_arch.into(),
            target_env: "musl".into(),
            components: embedded.clone(),
        };
        let frame = encode_embedded_agent_manifest_frame(&manifest).map_err(anyhow::Error::msg)?;
        let mut agent_bytes = b"\x7fELF-tcpquality-agent-fixture ".to_vec();
        agent_bytes.extend_from_slice(&frame);
        let mut entry = release_support::entry(
            "agent",
            "0.4.0",
            "apsp-agent",
            "raw",
            &agent_bytes,
            &agent_bytes,
        );
        entry.arch = target.into();
        entry.asset_name = canonical_asset_name(&entry)?;
        if target == "linux-musl-amd64" {
            amd64_sha = entry.binary_sha256.clone();
        }
        artifacts.push((entry, agent_bytes));
    }

    let mut proof = release_support::signed_release(artifacts.clone());
    let mut metadata: ReleaseMetadata = serde_json::from_str(&proof.metadata_json)?;
    metadata.tag = "agent-v0.4.0".into();
    proof.metadata_json = format!("{}\n", serde_json::to_string(&metadata)?);
    proof.checksums = proof
        .checksums
        .lines()
        .map(|line| {
            if line.ends_with("  release.json") {
                format!(
                    "{}  release.json\n",
                    release_support::hash(proof.metadata_json.as_bytes())
                )
            } else if line.ends_with("  install.sh") {
                format!("{}  install.sh\n", release_support::hash(installer))
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    proof.signature = release_support::sign(proof.checksums.as_bytes());

    let release_dir = panel
        .state
        .config
        .data_dir
        .join("artifacts/releases/agent-v0.4.0");
    std::fs::create_dir_all(&release_dir)?;
    release_support::install_proof(&release_dir, &proof);
    std::fs::write(release_dir.join("install.sh"), installer)?;
    for (entry, bytes) in artifacts {
        let dir = release_dir.join(&entry.name).join(&entry.version);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(&entry.arch), bytes)?;
    }

    let now = apsp_protocol::now_timestamp();
    sqlx::query(
        "UPDATE servers SET static_info=static_info || $2::jsonb,static_info_received_at=$4,capabilities=$3,last_seen=$4 WHERE id=$1",
    )
    .bind(server)
    .bind(json!({
        "os": "linux",
        "arch": "amd64",
        "libc": "musl",
        "agent_version": "0.4.0",
        "components": {
            "nodequality": node_ver,
            "tcpquality": TOOL_VERSION,
        },
        "component_digests": {
            "agent": amd64_sha,
            "nodequality": nodequality_sha,
            "tcpquality": amd64_sha,
        },
    }))
    .bind(json!([
        "diagnostic:nodequality",
        "diagnostic:nodequality-modes",
        "diagnostic:tcpquality",
        "diagnostic:tcpquality-native-v1",
        apsp_protocol::SELF_CONTAINED_AGENT_CAPABILITY,
        apsp_protocol::DIAGNOSTIC_SECTIONS_CAPABILITY,
        apsp_protocol::DIAGNOSTIC_SERVICE_CAPABILITY,
        apsp_protocol::DIAGNOSTIC_CPU_CEILING_CAPABILITY,
        apsp_protocol::DIAGNOSTIC_COMPLETION_CAPABILITY,
        apsp_protocol::DIAGNOSTIC_CANCEL_CAPABILITY,
        apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY,
    ]))
    .bind(now)
    .execute(&panel.state.pool)
    .await?;
    Ok(())
}
async fn probe(panel: &TestPanel, cookie: &str, server: i64, name: &str) -> Result<Value> {
    Ok(panel.admin(Method::POST,&format!("/api/servers/{server}/probes"),cookie,Some(probe_support::authorized(json!({
        "id":Uuid::nil(),"name":name,"kind":"tcp","target":"example.test","port":443,"interval_secs":60,"carrier":"fixture","enabled":true
    })))).await?.error_for_status()?.json().await?)
}
fn path(server: i64) -> String {
    format!("/api/servers/{server}/diagnostics/tcpquality")
}
fn region_path(server: i64, probe: &Value) -> String {
    format!(
        "/api/observability/tcp-quality/servers/{server}/targets/{}",
        probe["id"].as_str().unwrap()
    )
}

#[sqlx::test(migrations = "./migrations")]
async fn tcp_diagnostics_select_only_currently_authorized_targets(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, _ack) = panel
        .authenticated_device(&cookie, "permission filtered diagnostic")
        .await?;
    ready(&panel, server).await?;
    let allowed = probe(&panel, &cookie, server, "allowed fixture").await?;
    for expires_at in [None, Some(apsp_protocol::now_timestamp() - 1)] {
        let id = Uuid::new_v4();
        let mut spec = probe_support::authorized(
            json!({"id":id,"name":"unknown permission fixture","kind":"tcp","target":"unpermitted.example.test","port":443,"interval_secs":60,"carrier":"fixture","enabled":true}),
        );
        if let Some(expires_at) = expires_at {
            spec["monitor"]["authorization"]["expires_at"] = json!(expires_at);
        } else {
            spec["monitor"]["authorization"] = Value::Null;
        }
        sqlx::query("INSERT INTO network_probes(id,server_id,spec) VALUES($1,$2,$3)")
            .bind(id)
            .bind(server)
            .bind(spec)
            .execute(&panel.state.pool)
            .await?;
    }
    let targets: Vec<Value> = panel
        .admin(
            Method::GET,
            &format!("/api/observability/tcp-quality/servers/{server}/targets"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0]["id"], allowed["id"]);
    let job: Value = panel
        .admin(Method::POST, &path(server), &cookie, Some(json!({})))
        .await?
        .error_for_status()?
        .json()
        .await?;
    let snapshot: Value = serde_json::from_str(job["job"]["options"]["targets"].as_str().unwrap())?;
    assert_eq!(snapshot["targets"].as_array().unwrap().len(), 1);
    assert!(!serde_json::to_string(&snapshot)?.contains("unpermitted.example.test"));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn tcp_parameters_freeze_only_selected_configured_targets(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, _ack) = panel.authenticated_device(&cookie, "TCP scope").await?;
    ready(&panel, server).await?;
    let first = probe(&panel, &cookie, server, "东亚自有目标").await?;
    let second = probe(&panel, &cookie, server, "未标注地区目标").await?;
    assert_eq!(
        panel
            .admin(
                Method::PATCH,
                &region_path(server, &first),
                &cookie,
                Some(json!({"region":"east_asia"}))
            )
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        panel
            .admin(
                Method::PATCH,
                &region_path(server, &first),
                &cookie,
                Some(json!({}))
            )
            .await?
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let probe_id = Uuid::parse_str(first["id"].as_str().unwrap())?;
    let preserved: Option<String> =
        sqlx::query_scalar("SELECT region FROM tcpquality_target_regions WHERE probe_id=$1")
            .bind(probe_id)
            .fetch_optional(&panel.state.pool)
            .await?;
    assert_eq!(preserved.as_deref(), Some("east_asia"));
    assert_eq!(
        panel
            .admin(
                Method::PATCH,
                &region_path(server, &first),
                &cookie,
                Some(json!({"region":null}))
            )
            .await?
            .status(),
        StatusCode::OK
    );
    let cleared: Option<String> =
        sqlx::query_scalar("SELECT region FROM tcpquality_target_regions WHERE probe_id=$1")
            .bind(probe_id)
            .fetch_optional(&panel.state.pool)
            .await?;
    assert_eq!(cleared, None);
    panel
        .admin(
            Method::PATCH,
            &region_path(server, &first),
            &cookie,
            Some(json!({"region":"east_asia"})),
        )
        .await?
        .error_for_status()?;
    let legacy: Value = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{server}/probes"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        legacy
            .as_array()
            .unwrap()
            .iter()
            .all(|spec| spec.get("region").is_none())
    );
    let record: Value = panel
        .admin(
            Method::POST,
            &path(server),
            &cookie,
            Some(json!({"region":"east_asia","ip_version":"6","count":8,"concurrency":2})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let job = &record["job"];
    assert_eq!(job["timeout_secs"], 60);
    assert_eq!(job["version"], TOOL_VERSION);
    assert_eq!(
        job["resource_budget"],
        json!({"memory_max":64*1024*1024,"tasks_max":32,"cpu_weight":10,"io_weight":10,"oom_score_adjust":500})
    );
    let raw = job["options"]["targets"].as_str().unwrap();
    assert_eq!(
        job["options"]["target_digest"],
        format!("{:x}", Sha256::digest(raw.as_bytes()))
    );
    let snapshot: Value = serde_json::from_str(raw)?;
    assert_eq!(snapshot["schema"], 1);
    assert_eq!(snapshot["targets"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["targets"][0]["id"], first["id"]);
    assert_eq!(snapshot["targets"][0]["region"], "east_asia");
    assert_eq!(job["tcpquality"]["region"], "east_asia");
    assert_eq!(job["tcpquality"]["upload_enabled"], false);
    assert_eq!(job["tcpquality"]["ranking_enabled"], false);
    assert_eq!(job["tcpquality"]["speedtest_enabled"], false);
    assert_eq!(record["expected_sections"].as_array().unwrap().len(), 4);
    assert_eq!(
        panel
            .admin(
                Method::PATCH,
                &region_path(server, &first),
                &cookie,
                Some(json!({"region":"europe"}))
            )
            .await?
            .status(),
        StatusCode::OK
    );
    let mut changed = first.clone();
    changed["target"] = json!("changed.test");
    let probe_path = format!(
        "/api/servers/{server}/probes/{}",
        first["id"].as_str().unwrap()
    );
    assert_eq!(
        panel
            .admin(Method::PATCH, &probe_path, &cookie, Some(changed))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let mut metadata = first.clone();
    metadata["name"] = json!("更新后的名称");
    metadata["enabled"] = json!(false);
    panel
        .admin(Method::PATCH, &probe_path, &cookie, Some(metadata))
        .await?
        .error_for_status()?;
    let view: Value = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{server}/diagnostics"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(view["reports"][0]["job"]["options"]["targets"], raw);
    assert_eq!(
        view["reports"][0]["job"]["tcpquality"]["targets"][0]["target"],
        "example.test"
    );
    let inventory: Value = panel
        .admin(
            Method::GET,
            &format!("/api/observability/tcp-quality/servers/{server}/targets"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let unknown = inventory
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == second["id"])
        .unwrap();
    assert!(unknown["region"].is_null());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn tcp_and_nodequality_share_duplicates_cancel_confirmation_and_partial_history(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "TCP mutex").await?;
    ready(&panel, server).await?;
    let target = probe(&panel, &cookie, server, "本机受控目标").await?;
    let endpoint = path(server);
    let (first, duplicate) = tokio::join!(
        panel.admin(Method::POST, &endpoint, &cookie, Some(json!({}))),
        panel.admin(Method::POST, &endpoint, &cookie, Some(json!({})))
    );
    let (first, duplicate) = (first?, duplicate?);
    let created = if first.status() == StatusCode::CREATED {
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);
        first
    } else {
        assert_eq!(first.status(), StatusCode::CONFLICT);
        assert_eq!(duplicate.status(), StatusCode::CREATED);
        duplicate
    };
    let record: Value = created.json().await?;
    let id = record["id"].as_str().unwrap();
    let node = format!("/api/servers/{server}/diagnostics/nodequality");
    assert_eq!(
        panel
            .admin(Method::POST, &node, &cookie, Some(json!({"mode":"daily"})))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let chapter_name = format!(
        "tcp_target_{}",
        Uuid::parse_str(target["id"].as_str().unwrap())?.simple()
    );
    let section = json!({"id":id,"name":chapter_name,"text":"保存的独立 TCP 目标章节","complete":true,"revision":1,"collected_at":apsp_protocol::now_timestamp()});
    assert_eq!(
        panel
            .client
            .post(format!(
                "{}/api/agent/v1/diagnostics/{id}/sections",
                panel.base
            ))
            .bearer_auth(&ack.session_token)
            .json(&section)
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    let cancel_path = format!("/api/servers/{server}/diagnostics/{id}/cancel");
    let requested: Value = panel
        .admin(Method::POST, &cancel_path, &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(requested["status"], "cancel_requested");
    sqlx::query("UPDATE diagnostic_jobs SET expires_at=$2 WHERE id=$1")
        .bind(Uuid::parse_str(id)?)
        .bind(apsp_protocol::now_timestamp() - 1)
        .execute(&panel.state.pool)
        .await?;
    assert_eq!(
        panel
            .admin(Method::POST, &node, &cookie, Some(json!({"mode":"daily"})))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let pending: Value = panel
        .client
        .get(format!(
            "{}/api/agent/v1/diagnostics/cancellations",
            panel.base
        ))
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(pending[0]["job"]["id"], id);
    assert_eq!(pending[0]["job"]["plugin"], "tcpquality");
    let confirmation = json!({"server_id":server,"id":id,"plugin":"tcpquality","confirmed":true});
    assert_eq!(
        panel
            .client
            .post(format!(
                "{}/api/agent/v1/diagnostics/{id}/cancel-confirmation",
                panel.base
            ))
            .bearer_auth(&ack.session_token)
            .json(&confirmation)
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    let history: Value = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{server}/diagnostics"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(history["reports"][0]["status"], "cancelled");
    assert_eq!(history["reports"][0]["report_completeness"], "partial");
    assert_eq!(
        history["reports"][0]["sections"][0]["text"],
        "保存的独立 TCP 目标章节"
    );
    let node_record: Value = panel
        .admin(Method::POST, &node, &cookie, Some(json!({"mode":"daily"})))
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        panel
            .admin(Method::POST, &endpoint, &cookie, Some(json!({})))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let legacy: Value = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{server}/node-quality/reports"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(legacy["reports"].as_array().unwrap().len(), 1);
    assert_eq!(legacy["reports"][0]["id"], node_record["id"]);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn tcp_rejects_unapproved_parameters_missing_capabilities_and_foreign_targets(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "TCP validation")
        .await?;
    ready(&panel, server).await?;
    let endpoint = path(server);
    assert_eq!(
        panel
            .admin(Method::POST, &endpoint, &cookie, Some(json!({})))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    probe(&panel, &cookie, server, "可用目标").await?;
    for request in [
        json!({"count":256}),
        json!({"concurrency":8}),
        json!({"ip_version":"both"}),
        json!({"region":"all"}),
        json!({"allow_speedtest_staged":true}),
        json!({"no_rootfs":true}),
        json!({"no_rank_upload":false}),
        json!({"command":"echo x"}),
    ] {
        assert_eq!(
            panel
                .admin(Method::POST, &endpoint, &cookie, Some(request))
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        panel
            .client
            .post(format!("{}{endpoint}", panel.base))
            .bearer_auth(&ack.session_token)
            .json(&json!({}))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE servers SET capabilities=capabilities-'diagnostic:tcpquality-native-v1' WHERE id=$1").bind(server).execute(&panel.state.pool).await?;
    assert_eq!(
        panel
            .admin(Method::POST, &endpoint, &cookie, Some(json!({})))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    ready(&panel, server).await?;
    let other = panel.create_server(&cookie, "外国目标设备").await?;
    let foreign = probe(&panel, &cookie, other, "其他设备目标").await?;
    assert_eq!(
        panel
            .admin(
                Method::PATCH,
                &region_path(server, &foreign),
                &cookie,
                Some(json!({"region":"east_asia"}))
            )
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        panel
            .admin(
                Method::PATCH,
                &region_path(other, &foreign),
                &cookie,
                Some(json!({"region":"configured"}))
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    for index in 0..8 {
        probe(&panel, &cookie, server, &format!("超限目标{index}")).await?;
    }
    assert_eq!(
        panel
            .admin(Method::POST, &endpoint, &cookie, Some(json!({})))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM diagnostic_jobs WHERE server_id=$1")
        .bind(server)
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(count, 0);
    Ok(())
}

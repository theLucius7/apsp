#![forbid(unsafe_code)]

mod business_support;
#[path = "e2e_support/preflight.rs"]
mod deployment_preflight;
#[allow(dead_code)]
mod e2e_support;
#[allow(unused_imports)]
use business_support::release_fixture;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result};
use apsp_adapter_sdk::{Counter, Prepared};
use apsp_agent_core::{
    bundled::{BundledComponents, BundledItem, BundledPayload},
    config::Config as AgentConfig,
    fake::FakeServiceManager,
    identity,
    state::State,
    system::SystemOps,
    transport,
};
use apsp_protocol::{UsageBatch, UsageRecord};
use base64::{Engine, engine::general_purpose::STANDARD};
use e2e_support::{Harness, PanelAdapter, eventually};
use flate2::{Compression, write::GzEncoder};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::{borrow::Cow, fs, os::unix::fs::PermissionsExt, sync::Arc};
use tokio::task::JoinHandle;

fn bundled_singbox() -> Result<(Vec<u8>, BundledComponents)> {
    let binary = b"#!/bin/sh\nexit 0\n".to_vec();
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut header = tar::Header::new_gnu();
    header.set_size(binary.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder.append_data(&mut header, "demo", binary.as_slice())?;
    let archive = builder.into_inner()?.finish()?;
    let archive_sha256 = format!("{:x}", Sha256::digest(&archive));
    let mut components = BundledComponents::default();
    components.runtimes.insert(
        "singbox".into(),
        BundledItem {
            component_name: "sing-box".into(),
            binary_name: "demo".into(),
            version: apsp_traffic::RUNTIME_VERSION.into(),
            payload: BundledPayload::TarGz {
                archive_sha256,
                bytes: Cow::Owned(archive),
            },
        },
    );
    Ok((binary, components))
}

struct SelfContainedTask(JoinHandle<Result<()>>);

impl SelfContainedTask {
    fn start(
        config: AgentConfig,
        adapter: Arc<PanelAdapter>,
        bundled: BundledComponents,
        services: Arc<FakeServiceManager>,
    ) -> Self {
        Self(tokio::spawn(transport::run_with_components(
            config,
            vec![adapter],
            Vec::new(),
            bundled,
            Arc::new(SystemOps),
            services,
            "panel-test-agent",
        )))
    }

    async fn stop(self) -> Result<()> {
        self.0.abort();
        let mut task = self;
        match (&mut task.0).await {
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(error.into()),
            Ok(result) => result.context("agent stopped before cancellation"),
        }
    }
}

impl Drop for SelfContainedTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn published_configuration_usage_and_lost_ack_survive_agent_restart(
    pool: PgPool,
) -> Result<()> {
    let panel = Harness::start(pool.clone()).await?;
    let (expected_binary, bundled) = bundled_singbox()?;
    let config = panel.agent_config();
    let adapter = Arc::new(PanelAdapter::default());
    let services = Arc::new(FakeServiceManager::default());
    let server = panel
        .api(Method::POST, "/api/servers", json!({"name": "E2E server"}))
        .await?;
    let server_id = server["id"].as_i64().context("server id")?;
    let enrollment = panel
        .api(
            Method::POST,
            &format!("/api/servers/{server_id}/enrollment"),
            json!({}),
        )
        .await?;
    assert_eq!(
        identity::enroll(&config, enrollment["token"].as_str().context("token")?).await?,
        server_id
    );
    let policy_path = config
        .identity_dir
        .parent()
        .context("fixture Agent root")?
        .join("fleet-policy.json");
    fs::write(
        &policy_path,
        serde_json::to_vec(&apsp_protocol::fleet::AccessPolicy {
            runtime_inspection: true,
            ..Default::default()
        })?,
    )?;
    fs::set_permissions(&policy_path, fs::Permissions::from_mode(0o600))?;
    let original_key = fs::read(config.identity_dir.join("device.key"))?;
    let agent = SelfContainedTask::start(
        config.clone(),
        adapter.clone(),
        bundled.clone(),
        services.clone(),
    );
    eventually(
        "authenticated self-contained agent telemetry",
        10,
        || async {
            let server = panel
                .api(
                    Method::GET,
                    &format!("/api/servers/{server_id}"),
                    Value::Null,
                )
                .await?;
            Ok(server["online"] == true
                && server["static_info"]["arch"].is_string()
                && server["static_info"]["components"]["sing-box"] == apsp_traffic::RUNTIME_VERSION
                && server["static_info"]["runtime_version"].is_null())
        },
    )
    .await?;

    // Retired standalone enablement endpoint is rejected.
    assert_eq!(
        panel
            .client
            .post(format!(
                "{}/api/traffic/servers/{server_id}/enable",
                panel.base
            ))
            .header(reqwest::header::COOKIE, &panel.cookie)
            .json(&json!({}))
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );

    // Pause the Agent before saving business configuration: offline saving must queue
    // and publish automatically without requiring preflight or manual confirmation.
    agent.stop().await?;
    eventually(
        "Agent paused before offline first-time configuration",
        5,
        || async { Ok(!config.status_socket.exists()) },
    )
    .await?;

    let node = panel
        .api(
            Method::POST,
            "/api/traffic/nodes",
            json!({
                "name": "E2E node", "server_id": server_id,
                "public_host": "node.example.invalid", "sni": "www.example.com"
            }),
        )
        .await?;
    let node_id = node["id"].as_i64().context("node id")?;
    let user = panel
        .api(
            Method::POST,
            "/api/access/users",
            json!({"name": "E2E user"}),
        )
        .await?;
    let user_id = user["id"].as_i64().context("user id")?;
    let group = panel
        .api(
            Method::POST,
            "/api/access/policy-groups",
            json!({"name": "E2E group", "node_ids": [node_id], "chain_ids": []}),
        )
        .await?;
    panel
        .api(
            Method::PUT,
            &format!("/api/access/users/{user_id}/policy-groups"),
            json!({"group_ids": [group["id"]]}),
        )
        .await?;
    let access = panel
        .api(
            Method::GET,
            &format!("/api/access/users/{user_id}/accesses"),
            json!(null),
        )
        .await?[0]
        .clone();
    let stat_name = access["stat_name"]
        .as_str()
        .context("stat name")?
        .to_owned();
    let subscription = format!(
        "{}/sub/{}",
        panel.base,
        user["subscription_token"]
            .as_str()
            .context("subscription token")?
    );
    assert_eq!(
        panel
            .client
            .get(format!("{subscription}?format=singbox"))
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    assert!(
        panel
            .client
            .get(&subscription)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?
            .is_empty()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM deployments WHERE server_id=$1")
            .bind(server_id)
            .fetch_one(&pool)
            .await?,
        0
    );

    eventually(
        "debounced automatic deployment committed while Agent is paused without preflight",
        15,
        || async {
            let row: Option<(i64, i64)> = sqlx::query_as(
                "SELECT COALESCE((SELECT p.manifest_rev FROM runtime_publications p WHERE p.server_id=s.id),0),(SELECT count(*) FROM deployments d WHERE d.server_id=s.id AND d.module='singbox') FROM servers s WHERE s.id=$1",
            )
            .bind(server_id)
            .fetch_optional(&pool)
            .await?;
            Ok(row.is_some_and(|(revision, count)| revision > 0 && count == 1))
        },
    )
    .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM singbox_deployment_preflights WHERE server_id=$1"
        )
        .bind(server_id)
        .fetch_one(&pool)
        .await?,
        0,
        "ordinary node publication must not depend on preflight"
    );

    // Optional read-only preflight inspection remains available while confirm is retired.
    let preflight = deployment_preflight::observe(&panel, server_id).await?;
    let confirmed: (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT confirmed_at,confirmed_by FROM singbox_deployment_preflights WHERE id=$1 AND server_id=$2",
    )
    .bind(preflight)
    .bind(server_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(confirmed, (None, None));
    assert_eq!(
        panel
            .client
            .post(format!(
                "{}/api/traffic/servers/{server_id}/operations-view/preflight/confirm",
                panel.base
            ))
            .header(reqwest::header::COOKIE, &panel.cookie)
            .json(&json!({"id": preflight, "confirm": true}))
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert!(
        State::open(&config.state_db)?
            .get_json::<Prepared>("applied:singbox")?
            .is_none()
    );
    let agent = SelfContainedTask::start(
        config.clone(),
        adapter.clone(),
        bundled.clone(),
        services.clone(),
    );

    // The published bundle crosses authenticated WebSocket delivery and materializes
    // the Agent's embedded sing-box payload locally.
    eventually(
        "debounced deployment applied over WebSocket",
        15,
        || async {
            let runtime = panel
                .api(
                    Method::GET,
                    &format!("/api/traffic/servers/{server_id}"),
                    Value::Null,
                )
                .await?;
            Ok(runtime["configured"] == true
                && runtime["status"]["state"] == "ready"
                && runtime["runtime_version"] == apsp_traffic::RUNTIME_VERSION)
        },
    )
    .await?;
    let actual = panel
        .api(
            Method::GET,
            &format!("/api/servers/{server_id}"),
            Value::Null,
        )
        .await?;
    assert_eq!(actual["static_info"]["os"], std::env::consts::OS);
    if std::env::consts::OS != "linux" {
        assert!(actual["static_info"].get("runtime_libc").is_none());
        assert!(actual["static_info"].get("libc").is_none());
        for capability in [
            apsp_protocol::fleet::OPERATIONS_CAPABILITY,
            apsp_protocol::fleet::RUNTIME_PREFLIGHT_CAPABILITY,
        ] {
            assert!(
                !actual["capabilities"]
                    .as_array()
                    .context("actual Agent capabilities")?
                    .iter()
                    .any(|value| value == capability)
            );
        }
    }
    let local = State::open(&config.state_db)?;
    let applied: Prepared = local
        .get_json("applied:singbox")?
        .context("applied local revision")?;
    assert!(local.pending_intents()?.is_empty());
    drop(local);
    assert_eq!(fs::read(&applied.spec.binary_path)?, expected_binary);
    assert_eq!(
        fs::read_link(config.runtime_root.join("sing-box@main/current"))?,
        applied.spec.revision_dir
    );
    let runtime_config: Value =
        serde_json::from_slice(&fs::read(applied.spec.revision_dir.join("config.json"))?)?;
    assert_eq!(runtime_config["inbounds"][0]["users"][0]["name"], stat_name);
    assert_eq!(
        runtime_config["inbounds"][0]["users"][0]["uuid"],
        access["uuid"]
    );
    assert_eq!(
        fs::metadata(&config.status_socket)?.permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(config.status_socket.parent().unwrap())?
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let status = transport::status(&config.status_socket).await?;
    assert_eq!(status["connected"], true);
    assert_eq!(status["applied"]["singbox"], applied.spec.revision);
    assert_eq!(status["healthy"]["singbox"], true);
    let client_config: Value = panel
        .client
        .get(format!("{subscription}?format=singbox"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(client_config["outbounds"][1]["uuid"], access["uuid"]);
    let links = panel
        .client
        .get(&subscription)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let links = String::from_utf8(STANDARD.decode(links)?)?;
    assert!(links.contains(access["uuid"].as_str().context("access uuid")?));

    *adapter.0.counters.lock().unwrap() = vec![Counter {
        stat_name: stat_name.clone(),
        uplink: 100,
        downlink: 200,
    }];
    // The production 30 s sampler and 15 s outbox retry provide the first batch and ACK.
    eventually(
        "periodic usage batch committed and acknowledged",
        48,
        || async {
            let totals = panel
                .api(Method::GET, "/api/access/usage", Value::Null)
                .await?;
            Ok(totals["uplink"] == "100"
                && totals["downlink"] == "200"
                && transport::status(&config.status_socket).await?["pending_batches"] == 0)
        },
    )
    .await?;
    assert_usage(&panel, user_id, node_id).await?;
    agent.stop().await?;
    eventually("agent children and status socket stopped", 5, || async {
        Ok(!config.status_socket.exists())
    })
    .await?;

    // Requeue the exact committed batch to model a process dying before its ACK was durable.
    let row = sqlx::query("SELECT b.epoch,b.seq::text AS seq,r.period_start,r.period_end,r.stat_name,r.uplink::text AS uplink,r.downlink::text AS downlink FROM usage_batches b JOIN usage_records r USING(server_id,epoch,seq) WHERE b.server_id=$1")
        .bind(server_id).fetch_one(&pool).await?;
    let batch = UsageBatch {
        epoch: row.get("epoch"),
        seq: row.get::<String, _>("seq").parse()?,
        period_start: row.get("period_start"),
        period_end: row.get("period_end"),
        records: vec![UsageRecord {
            stat_name: row.get("stat_name"),
            uplink: row.get::<String, _>("uplink").parse()?,
            downlink: row.get::<String, _>("downlink").parse()?,
        }],
    };
    {
        let connection = rusqlite::Connection::open(&config.state_db)?;
        connection.execute("INSERT INTO usage_outbox(epoch,seq,batch,acknowledged) VALUES(?1,?2,?3,0) ON CONFLICT(epoch,seq) DO UPDATE SET acknowledged=0",
            rusqlite::params![batch.epoch.to_string(), batch.seq.to_string(), serde_json::to_string(&batch)?])?;
    }
    assert_eq!(State::open(&config.state_db)?.pending_usage_count()?, 1);
    let actions_before_restart = services.actions.lock().unwrap().clone();
    let restarted = SelfContainedTask::start(
        config.clone(),
        adapter.clone(),
        bundled.clone(),
        services.clone(),
    );
    eventually(
        "restarted agent retransmission acknowledged",
        10,
        || async {
            if !config.status_socket.exists() {
                return Ok(false);
            }
            let status = transport::status(&config.status_socket).await?;
            Ok(status["connected"] == true && status["pending_batches"] == 0)
        },
    )
    .await?;
    assert_usage(&panel, user_id, node_id).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM usage_batches WHERE server_id=$1")
            .bind(server_id)
            .fetch_one(&pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM usage_records WHERE server_id=$1")
            .bind(server_id)
            .fetch_one(&pool)
            .await?,
        1
    );
    assert_eq!(
        fs::read(config.identity_dir.join("device.key"))?,
        original_key
    );
    assert_eq!(*services.actions.lock().unwrap(), actions_before_restart);
    assert_eq!(
        State::open(&config.state_db)?.get_json::<Prepared>("applied:singbox")?,
        Some(applied)
    );

    // Deleting the last node deactivates sing-box and keeps it stopped across Agent restarts.
    panel
        .api(
            Method::DELETE,
            &format!("/api/traffic/nodes/{node_id}"),
            Value::Null,
        )
        .await?;
    eventually("last node removal deactivates runtime", 15, || async {
        let runtime = panel
            .api(
                Method::GET,
                &format!("/api/traffic/servers/{server_id}"),
                Value::Null,
            )
            .await?;
        Ok(runtime["configured"] == false
            && runtime["status"]["target_rev"] == 0
            && runtime["status"]["applied_rev"] == 0
            && runtime["runtime_version"].is_null())
    })
    .await?;
    assert!(
        State::open(&config.state_db)?
            .get_json::<Prepared>("applied:singbox")?
            .is_none()
    );
    restarted.stop().await?;
    eventually("restarted agent children stopped", 5, || async {
        Ok(!config.status_socket.exists())
    })
    .await?;

    let actions_after_stop = services.actions.lock().unwrap().clone();
    let verify_idle = SelfContainedTask::start(config.clone(), adapter, bundled, services.clone());
    eventually(
        "idle agent reconnects without reviving singbox",
        10,
        || async {
            if !config.status_socket.exists() {
                return Ok(false);
            }
            let status = transport::status(&config.status_socket).await?;
            Ok(status["connected"] == true && status["applied"].get("singbox").is_none())
        },
    )
    .await?;
    assert_eq!(*services.actions.lock().unwrap(), actions_after_stop);
    verify_idle.stop().await?;
    eventually("idle agent stopped", 5, || async {
        Ok(!config.status_socket.exists())
    })
    .await?;
    Ok(())
}

async fn assert_usage(panel: &Harness, user_id: i64, node_id: i64) -> Result<()> {
    let usage = panel
        .api(Method::GET, "/api/access/usage", Value::Null)
        .await?;
    assert_eq!(usage["uplink"], "100");
    assert_eq!(usage["downlink"], "200");
    assert_eq!(usage["total"], "300");
    assert_eq!(usage["by_user"].as_array().context("user totals")?.len(), 1);
    assert_eq!(usage["by_user"][0]["user_id"], user_id);
    assert_eq!(usage["by_user"][0]["uplink"], "100");
    assert_eq!(usage["by_user"][0]["downlink"], "200");
    assert_eq!(usage["by_node"].as_array().context("node totals")?.len(), 1);
    assert_eq!(usage["by_node"][0]["node_id"], node_id);
    assert_eq!(usage["by_node"][0]["uplink"], "100");
    assert_eq!(usage["by_node"][0]["downlink"], "200");
    Ok(())
}

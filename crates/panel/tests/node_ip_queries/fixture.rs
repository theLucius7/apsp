use super::*;
use apsp_protocol::{Hello, Message, PROTOCOL_VERSION};
use std::collections::BTreeMap;
pub const VERSION: &str =
    apsp_observability::diagnostic_tools::nodequality::node_queries::NODE_QUERY_VERSION;

pub async fn prepare(panel: &TestPanel, server: i64) -> Result<()> {
    use apsp_protocol::{
        bundled::{
            EmbeddedAgentManifest, EmbeddedComponentRecord, encode_embedded_agent_manifest_frame,
        },
        release::{ReleaseMetadata, canonical_asset_name},
    };

    let installer = b"#!/bin/sh\n# APSP_BOOTSTRAP_AGENT_SOURCE=preloaded-github-v1\nexit 0\n";
    let binary = b"TEST_ONLY signed node query fixture";
    let archive = release_fixture::archive("nodequality", binary)?;
    let nodequality_sha = release_support::hash(&archive);
    let mut embedded = BTreeMap::new();
    embedded.insert(
        "nodequality".into(),
        EmbeddedComponentRecord {
            format: "tar.gz".into(),
            inventory_sha256: None,
            sha256: Some(nodequality_sha.clone()),
            subcommand: None,
            version: VERSION.into(),
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
        let mut agent_bytes = b"\x7fELF-node-ip-queries-agent ".to_vec();
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

    apsp_resources::device_channel::process_message(
        &panel.state,
        server,
        Message::Hello(Hello {
            agent_version: "0.4.0".into(),
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec![
                "diagnostic:nodequality",
                "diagnostic:nodequality-modes",
                "diagnostic:nodequality-node-query",
                apsp_protocol::SELF_CONTAINED_AGENT_CAPABILITY,
                apsp_protocol::DIAGNOSTIC_CANCEL_CAPABILITY,
                apsp_protocol::DIAGNOSTIC_SECTIONS_CAPABILITY,
                apsp_protocol::DIAGNOSTIC_SERVICE_CAPABILITY,
                apsp_protocol::DIAGNOSTIC_CPU_CEILING_CAPABILITY,
                apsp_protocol::DIAGNOSTIC_COMPLETION_CAPABILITY,
                apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY,
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            applied: BTreeMap::new(),
        }),
    )
    .await?;
    let now = apsp_protocol::now_timestamp();
    sqlx::query(
        "UPDATE servers SET static_info=static_info || $2::jsonb,static_info_received_at=$3,last_seen=$3 WHERE id=$1",
    )
    .bind(server)
    .bind(json!({
        "os": "linux",
        "arch": "amd64",
        "libc": "musl",
        "ip_addresses": ["1.1.1.1", "127.0.0.1"],
        "agent_version": "0.4.0",
        "components": { "nodequality": VERSION },
        "component_digests": { "agent": amd64_sha, "nodequality": nodequality_sha },
    }))
    .bind(now)
    .execute(&panel.state.pool)
    .await?;
    Ok(())
}
pub async fn create(panel: &TestPanel, server: i64, cookie: &str) -> Result<Value> {
    Ok(panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/ip-quality/node-query"),
            cookie,
            Some(json!({"ip_version":"both"})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?)
}
pub async fn upload(
    panel: &TestPanel,
    token: &str,
    id: &str,
    receipt: &Value,
) -> Result<StatusCode> {
    Ok(panel
        .client
        .post(format!(
            "{}/api/agent/v1/diagnostics/{id}/sections",
            panel.base
        ))
        .bearer_auth(token)
        .json(receipt)
        .send()
        .await?
        .status())
}
pub async fn view(panel: &TestPanel, server: i64, cookie: &str) -> Result<Value> {
    Ok(panel
        .admin(
            Method::GET,
            &format!("/api/servers/{server}/ip-quality"),
            cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?)
}
pub fn section(
    id: &str,
    at: i64,
    revision: u64,
    complete: bool,
    failure: Option<(&str, Option<u16>)>,
) -> Value {
    let rows: Vec<_> = [("ipregistry-node", "ipregistry-v1", "https://api.ipregistry.co"),
        ("dbip-node", "dbip-v2", "https://api.db-ip.com/v2")].into_iter().map(|(provider, database, source)| {
            let omitted = failure.is_some_and(|(kind, _)| kind == "not_attempted");
            json!({"provider":provider,"database":database,"source":source,"target_ip":"1.1.1.1","execution":"node",
                "available":!omitted,"observed_ip":if omitted {Value::Null} else {json!("1.1.1.1")},
                "attempted_at":if omitted {Value::Null} else {json!(at)},"elapsed_ms":if omitted {Value::Null} else {json!(0)},
                "data":if failure.is_some() {Value::Null} else if provider=="ipregistry-node" {
                    json!({"ip":"1.1.1.1","type":"IPv4","security":{"is_proxy":false}})
                } else {json!({"ipAddress":"1.1.1.1","latitude":0,"isProxy":false})},
                "error":failure.map(|(kind,status)| json!({"kind":kind,"message":"TEST_ONLY 未完成正式查询，信息未知","http_status":status}))})
        }).collect();
    let body = json!({"schema":"apsp.node-ip-quality.v1","job_id":id,"execution":"node","ip_version":"both",
        "started_at":at,"finished_at":at,"ips":["1.1.1.1"],"results":rows,
        "streaming":{"execution":"node","status":"unknown","reason":"TEST_ONLY 正式流媒体认证未配置，信息未知"}});
    json!({"id":id,"name":"ip_quality","text":body.to_string(),"complete":complete,"revision":revision,"collected_at":at})
}
fn datasets(quality: &Value) -> impl Iterator<Item = &Value> {
    quality["quality"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["databases"][0])
}
pub fn assert_current_false_zero(quality: &Value, at: i64) {
    assert_eq!(quality["quality"].as_array().unwrap().len(), 2);
    for dataset in datasets(quality) {
        assert_eq!(dataset["status"], "succeeded");
        assert_eq!(dataset["historical"], false);
        assert_eq!(dataset["last_success_at"], at);
        assert_eq!(dataset["execution"], "node");
        assert_false_zero(dataset);
    }
}
pub fn assert_history_false_zero(quality: &Value, at: i64, kind: &str) {
    assert_eq!(quality["quality"].as_array().unwrap().len(), 2);
    for dataset in datasets(quality) {
        assert_eq!(dataset["status"], "failed");
        assert_eq!(dataset["historical"], true);
        assert_eq!(dataset["last_success_at"], at);
        assert_eq!(dataset["last_error"]["kind"], kind);
        assert_false_zero(dataset);
    }
}
fn assert_false_zero(dataset: &Value) {
    let fields = dataset["fields"].as_array().unwrap();
    assert_eq!(
        fields
            .iter()
            .find(|field| field["label"] == "代理")
            .unwrap()["value"],
        false
    );
    if dataset["database"] == "dbip-v2" {
        assert_eq!(
            fields
                .iter()
                .find(|field| field["label"] == "纬度")
                .unwrap()["value"],
            0
        );
    }
}

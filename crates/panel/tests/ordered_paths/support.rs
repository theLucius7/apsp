use super::*;

pub(super) const ROOT: &str = "/api/plugins/sing-box";
pub(super) struct Fixture {
    pub panel: TestPanel,
    pub cookie: String,
    pub servers: Vec<i64>,
    pub nodes: Vec<i64>,
    pub source: i64,
    pub external: Value,
}

pub(super) async fn api(
    panel: &TestPanel,
    cookie: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
    status: StatusCode,
) -> Result<Value> {
    let response = panel
        .admin(method, &format!("{ROOT}{path}"), cookie, body)
        .await?;
    ensure!(
        response.status() == status,
        "{path}: unexpected HTTP status {}",
        response.status()
    );
    if status == StatusCode::NO_CONTENT {
        return Ok(Value::Null);
    }
    Ok(response.json().await?)
}

pub(super) async fn fixture(pool: PgPool) -> Result<Fixture> {
    let panel = TestPanel::start_with_public_url(pool, Some("https://panel.example")).await?;
    let archive = release_fixture::archive("sing-box", b"TEST_ONLY native binary never executed")?;
    release_fixture::write(
        &panel.state.config.data_dir,
        "sing-box",
        "1.14.2",
        "sing-box",
        &archive,
        b"TEST_ONLY native binary never executed",
        "tar.gz",
    )?;
    let cookie = panel.admin_cookie().await?;
    let mut servers = Vec::new();
    let mut nodes = Vec::new();
    for name in ["A", "M", "B"] {
        let server = panel.create_server(&cookie, name).await?;
        sqlx::query("UPDATE servers SET capabilities=$2,static_info=$3 WHERE id=$1")
            .bind(server).bind(json!([RUNTIME_CHECKPOINT_CAPABILITY,RUNTIME_RECOVERY_BARRIER_CAPABILITY,RUNTIME_PATH_PROBE_CAPABILITY]))
            .bind(json!({"os":"linux","arch":apsp_protocol::release::native_arch()?,"libc":"gnu","runtime_libc":"gnu"}))
            .execute(&panel.state.pool).await?;
        let node = id(&panel.create_node(&cookie, server, name).await?)?;
        api(
            &panel,
            &cookie,
            Method::PATCH,
            &format!("/nodes/{node}"),
            Some(json!({"public_host":format!("{}.example.net",name.to_lowercase())})),
            StatusCode::OK,
        )
        .await?;
        nodes.push(node);
        servers.push(server);
    }
    let receipt = api(&panel, &cookie, Method::POST, "/ordered-subscription-sources", Some(json!({
        "request_id":Uuid::new_v4(),"name":"Controlled X","input":{"kind":"inline","content":content("TEST_ONLY external password")}
    })), StatusCode::ACCEPTED).await?;
    let source = receipt["source_id"].as_i64().context("source ID")?;
    apsp_panel::plugins::singbox::subscription_sources::worker::run_once(&panel.state).await?;
    let page = api(
        &panel,
        &cookie,
        Method::GET,
        &format!("/ordered-subscription-sources/{source}/nodes"),
        None,
        StatusCode::OK,
    )
    .await?;
    let external = page["nodes"][0].clone();
    ensure!(
        external["selectable"] == true,
        "controlled external fixture is not selectable"
    );
    Ok(Fixture {
        panel,
        cookie,
        servers,
        nodes,
        source,
        external,
    })
}

pub(super) fn content(password: &str) -> String {
    json!({"outbounds":[{"type":"shadowsocks","tag":"X","server":"external.example.net","server_port":23456,"method":"aes-128-gcm","password":password}]}).to_string()
}

pub(super) fn item(fixture: &Fixture, mode: &str, four_hops: bool) -> Value {
    let mut hops = Vec::new();
    if four_hops {
        hops.push(json!({"kind":"managed","node_id":fixture.nodes[1]}));
    }
    hops.push(json!({"kind":"subscription","source_id":fixture.source,"external_node_id":fixture.external["id"],
        "node_version_id":fixture.external["version_id"],"update_mode":mode}));
    hops.push(json!({"kind":"managed","node_id":fixture.nodes[2]}));
    json!({"name":if four_hops{"A-M-X-B"}else{"A-X-B"},"entry":{"mode":"new","server_id":fixture.servers[0],
        "public_host":"entry.example.net","sni":"www.example.com"},"hops":hops})
}

pub(super) async fn create(fixture: &Fixture, mode: &str, four_hops: bool) -> Result<Value> {
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::POST,
        "/chains/ordered-batch",
        Some(json!({"request_id":Uuid::new_v4(),
        "items":[item(fixture,mode,four_hops)]})),
        StatusCode::CREATED,
    )
    .await
}
pub(super) fn chain(receipt: &Value) -> Result<i64> {
    receipt["chain_ids"][0].as_i64().context("chain ID")
}
pub(super) async fn phase(fixture: &Fixture, chain: i64) -> Result<String> {
    Ok(
        sqlx::query_scalar("SELECT phase FROM singbox_chains WHERE id=$1")
            .bind(chain)
            .fetch_one(&fixture.panel.state.pool)
            .await?,
    )
}
pub(super) async fn resource(fixture: &Fixture, chain: i64) -> Result<Value> {
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::GET,
        &format!("/ordered-proxy-resources/chain/{chain}"),
        None,
        StatusCode::OK,
    )
    .await
}

pub(super) async fn authorize(fixture: &Fixture, chain: i64) -> Result<Value> {
    let user = fixture
        .panel
        .create_user(
            &fixture.cookie,
            &format!("Controlled terminal user {chain}"),
        )
        .await?;
    let group = api(
        &fixture.panel,
        &fixture.cookie,
        Method::POST,
        "/policy-groups",
        Some(json!({"name":format!("Ordered paths {chain}"),
        "node_ids":[],"chain_ids":[chain]})),
        StatusCode::CREATED,
    )
    .await?;
    api(
        &fixture.panel,
        &fixture.cookie,
        Method::PUT,
        &format!("/users/{}/policy-groups", id(&user)?),
        Some(json!({"group_ids":[id(&group)?]})),
        StatusCode::OK,
    )
    .await?;
    Ok(user)
}

/// Device observations are explicit TEST_ONLY facts; this never runs a native process.
pub(super) async fn confirm_devices(state: &AppState) -> Result<()> {
    let manifests:Vec<(i64,Value)>=sqlx::query_as("SELECT s.id,s.static_info FROM servers s WHERE s.deleted_at IS NULL AND s.dirty_at IS NULL AND EXISTS(SELECT 1 FROM deployments d WHERE d.server_id=s.id AND d.module='singbox') ORDER BY s.id").fetch_all(&state.pool).await?;
    for (server, info) in manifests {
        ensure!(
            apsp_panel::plugins::singbox::agent::manifest_module(state, server, &info)
                .await?
                .is_some(),
            "TEST_ONLY signed manifest preparation is missing"
        );
    }
    let servers: Vec<i64> = sqlx::query_scalar("SELECT m.server_id FROM server_module_status m JOIN servers s ON s.id=m.server_id WHERE m.module='singbox' AND m.target_rev>0 AND (NOT m.healthy OR NOT EXISTS(SELECT 1 FROM runtime_module_checkpoints r WHERE r.server_id=m.server_id AND r.module=m.module AND (r.checkpoint_json->'binding'->>'revision')::bigint=m.target_rev AND r.checkpoint_json->>'healthy'='true')) AND s.dirty_at IS NULL AND s.deleted_at IS NULL ORDER BY m.server_id")
        .fetch_all(&state.pool).await?;
    for server in servers {
        let request = runtime_control::request_checkpoint(state, server, "singbox").await?;
        let revision = request.expected.revision;
        let observed = RuntimeCheckpoint {
            binding: request.expected.clone(),
            activation_id: Uuid::from_u128((server as u128) * 100000 + revision as u128),
            instance_id: format!("backend:TEST_ONLY-{server}-{revision}"),
            healthy: true,
        };
        agent_api::process_message(
            state,
            server,
            Message::RuntimeCheckpointResult(RuntimeCheckpointResult {
                request_id: request.request_id,
                request_digest: request.digest()?,
                observed: Some(observed),
                success: true,
                error: None,
            }),
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn finish_controls(state: &AppState, probes_successful: bool) -> Result<()> {
    let requests: Vec<(i64,String,Value)> = sqlx::query_as("SELECT server_id,kind,request_json FROM runtime_control_requests WHERE state='pending' AND kind IN ('probe','barrier') ORDER BY created_at,request_id")
        .fetch_all(&state.pool).await?;
    for (server, kind, payload) in requests {
        let message = if kind == "probe" {
            let request: RuntimePathProbeRequest = serde_json::from_value(payload)?;
            Message::RuntimePathProbeResult(RuntimePathProbeResult {
                request_id: request.request_id,
                request_digest: request.digest()?,
                probe_id: request.probe_id,
                observed: probes_successful.then_some(request.expected),
                elapsed_ms: probes_successful.then_some(17),
                success: probes_successful,
                error: (!probes_successful).then(|| "TEST_ONLY controlled probe failed".into()),
            })
        } else {
            let request: RuntimeRecoveryBarrierRequest = serde_json::from_value(payload)?;
            Message::RuntimeRecoveryBarrierResult(RuntimeRecoveryBarrierResult {
                request_id: request.request_id,
                request_digest: request.digest()?,
                observed: Some(request.expected),
                minimum_revision: Some(request.minimum_revision),
                pending_intents_clear: true,
                success: true,
                error: None,
            })
        };
        agent_api::process_message(state, server, message).await?;
    }
    Ok(())
}

pub(super) async fn advance(fixture: &Fixture, chain: i64, terminal: &str) -> Result<Vec<String>> {
    let mut phases = Vec::new();
    for _ in 0..32 {
        let current = phase(fixture, chain).await?;
        phases.push(current.clone());
        if current == terminal {
            return Ok(phases);
        }
        fixture.panel.publish_now().await?;
        confirm_devices(&fixture.panel.state).await?;
        apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state)
            .await?;
        finish_controls(&fixture.panel.state, true).await?;
        apsp_panel::plugins::singbox::ordered_paths::reconcile_pending(&fixture.panel.state)
            .await?;
    }
    anyhow::bail!("ordered fixture did not reach {terminal}; phases: {phases:?}")
}

pub(super) async fn subscription(fixture: &Fixture, user: &Value) -> Result<Value> {
    let token = user["subscription_token"]
        .as_str()
        .context("existing subscription token")?;
    let published = reqwest::Url::parse(
        user["subscription_url"]
            .as_str()
            .context("returned subscription URL")?,
    )?;
    ensure!(
        published.path() == format!("/sub/{token}")
            && published.query().is_none()
            && published.fragment().is_none(),
        "returned subscription URL must preserve the original token path"
    );
    // The signed public origin is TEST_ONLY; exercise the returned route on this
    // fixture's actual loopback HTTP server rather than contacting that origin.
    let response = fixture
        .panel
        .client
        .get(format!(
            "{}{}?format=singbox",
            fixture.panel.base,
            published.path()
        ))
        .send()
        .await?;
    if response.status() == StatusCode::CONFLICT {
        let rejected: Value = response.json().await?;
        ensure!(
            rejected["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty()),
            "blocked public subscription must explain its unavailable result"
        );
        let preview = api(
            &fixture.panel,
            &fixture.cookie,
            Method::GET,
            &format!("/users/{}/subscription?format=singbox", id(user)?),
            None,
            StatusCode::OK,
        )
        .await?;
        ensure!(
            preview["status"] == "empty"
                && preview["content"].is_null()
                && preview["ready_nodes"].as_array().is_some_and(Vec::is_empty),
            "public 409 must correspond to an actually empty applied subscription"
        );
        return Ok(rejected);
    }
    ensure!(
        response.status() == StatusCode::OK,
        "public subscription returned unexpected HTTP status {}",
        response.status()
    );
    Ok(response.json().await?)
}
pub(super) fn subscription_nodes(value: &Value) -> Vec<&Value> {
    if value.get("outbounds").is_none() {
        assert!(
            value["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty()),
            "only a confirmed blocked public response can omit outbounds"
        );
        return Vec::new();
    }
    value["outbounds"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|outbound| {
            outbound["type"].as_str().is_some_and(|kind| {
                matches!(
                    kind,
                    "vless"
                        | "vmess"
                        | "trojan"
                        | "shadowsocks"
                        | "hysteria2"
                        | "tuic"
                        | "anytls"
                        | "socks"
                        | "http"
                )
            })
        })
        .collect()
}
pub(super) fn no_secrets(value: &Value) {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                assert!(
                    ![
                        "private_key",
                        "password",
                        "credential",
                        "relay_uuid",
                        "secret",
                        "normalized_config",
                        "outbound",
                        "snapshot",
                        "url",
                        "auth_headers",
                        "subscription_token"
                    ]
                    .contains(&name.as_str())
                );
                no_secrets(value);
            }
        }
        Value::Array(values) => values.iter().for_each(no_secrets),
        Value::String(value) => assert!(!value.contains("TEST_ONLY external password")),
        _ => {}
    }
}

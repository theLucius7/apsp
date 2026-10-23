#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result, ensure};
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const ROOT: &str = "/api/plugins/sing-box";

async fn call(
    panel: &TestPanel,
    cookie: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value> {
    let response = panel
        .admin(method, &format!("{ROOT}{path}"), cookie, body)
        .await?;
    let status = response.status();
    let text = response.text().await?;
    ensure!(status.is_success(), "{path}: {status}: {text}");
    Ok(if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text)?
    })
}

async fn create(
    panel: &TestPanel,
    cookie: &str,
    server: i64,
    kind: &str,
    settings: Value,
) -> Result<Value> {
    let tls = matches!(kind, "hysteria2" | "tuic" | "anytls" | "naive");
    let mut protocol = json!({"type":kind});
    if tls {
        protocol["tls"] = json!({"mode":"acme","email":"admin@example.com","challenge":"http-01"});
    }
    call(
        panel,
        cookie,
        Method::POST,
        "/nodes",
        Some(json!({
            "name":kind,"server_id":server,"public_host":"node.example.com",
            "sni":if tls || kind == "vless-reality" { "node.example.com" } else { "" },
            "protocol_config":protocol,"settings":settings,
        })),
    )
    .await
}

async fn patch(panel: &TestPanel, cookie: &str, node: i64, settings: Value) -> Result<Value> {
    call(
        panel,
        cookie,
        Method::PATCH,
        &format!("/nodes/{node}"),
        Some(json!({"settings":settings})),
    )
    .await
}

async fn stored(pool: &PgPool, node: i64) -> Result<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('settings',settings,'protocol_config',protocol_config,'private_key',private_key,'public_key',public_key,'short_id',short_id) FROM nodes WHERE id=$1").bind(node).fetch_one(pool).await?)
}

#[sqlx::test]
async fn optional_settings_preserve_omitted_values_and_clear_explicit_nulls(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Settings").await?;
    panel.enable_plugin(&cookie, server).await?;
    let values = json!({"public_port":8443,"tcp_keep_alive_seconds":120,"tcp_keep_alive_interval_seconds":30,
        "tls_min_version":"1.2","tls_max_version":"1.3","tls_handshake_timeout_seconds":15,
        "anytls":{"idle_session_check_seconds":30,"padding_scheme":["stop=2","0=30-30","1=100-400,c,500-1000"]}});
    let created = create(&panel, &cookie, server, "anytls", values.clone()).await?;
    let node = id(&created)?;
    for (key, value) in values.as_object().unwrap() {
        if key != "anytls" {
            assert_eq!(&created["settings"][key], value, "{key}");
        }
    }
    let original = stored(&pool, node).await?;
    let renamed = call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/nodes/{node}"),
        Some(json!({"name":"Preserved"})),
    )
    .await?;
    assert_eq!(renamed["settings"], created["settings"]);
    assert_eq!(stored(&pool, node).await?, original);
    let empty = patch(&panel, &cookie, node, json!({})).await?;
    assert_eq!(empty["settings"], created["settings"]);
    let cleared = patch(
        &panel,
        &cookie,
        node,
        json!({"public_port":null,"tcp_keep_alive_seconds":null,
        "tcp_keep_alive_interval_seconds":null,"tls_min_version":null,"tls_max_version":null,
        "tls_handshake_timeout_seconds":null,"anytls":{}}),
    )
    .await?;
    for key in [
        "public_port",
        "tcp_keep_alive_seconds",
        "tcp_keep_alive_interval_seconds",
        "tls_min_version",
        "tls_max_version",
        "tls_handshake_timeout_seconds",
    ] {
        assert!(cleared["settings"][key].is_null(), "{key}");
    }
    assert_eq!(cleared["settings"]["anytls"]["padding_scheme"], json!([]));
    assert!(cleared["settings"]["anytls"]["idle_session_check_seconds"].is_null());
    assert_eq!(
        stored(&pool, node).await?["protocol_config"],
        original["protocol_config"]
    );
    Ok(())
}

#[sqlx::test]
async fn protocol_options_roundtrip_preserve_credentials_and_reach_the_correct_side(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Options").await?;
    panel.enable_plugin(&cookie, server).await?;
    let user = panel.create_user(&cookie, "Subscriber").await?;
    let user_id = id(&user)?;
    let cases = [
        (
            "hysteria2",
            json!({"hysteria2":{"ignore_client_bandwidth":true,"obfs_enabled":true,"bbr_profile":"conservative",
            "masquerade":{"status_code":200,"content_type":"text/plain","content":"TEST_ONLY decoy response"}}}),
        ),
        (
            "tuic",
            json!({"tuic":{"udp_relay_mode":"udp-over-stream","congestion_control":"bbr"}}),
        ),
        (
            "anytls",
            json!({"anytls":{"padding_scheme":["stop=1","0=30-30"],"min_idle_session":2}}),
        ),
        (
            "shadowsocks2022",
            json!({"shadowsocks":{"udp_over_tcp":true,"multiplex":{"enabled":true,"padding":true,"protocol":"smux","max_connections":4,"min_streams":2}}}),
        ),
        (
            "snell-v6",
            json!({"snell":{"mode":"unshaped","reuse":true}}),
        ),
        (
            "naive",
            json!({"tls_min_version":"1.2","tls_max_version":"1.3","tls_handshake_timeout_seconds":15}),
        ),
    ];
    let mut nodes = Vec::new();
    for (kind, settings) in cases {
        let created = create(&panel, &cookie, server, kind, settings.clone()).await?;
        let node = id(&created)?;
        panel.grant(&cookie, user_id, node).await?;
        let before = stored(&pool, node).await?;
        let identity: (Uuid, String) =
            sqlx::query_as("SELECT uuid,credential FROM accesses WHERE node_id=$1 AND user_id=$2")
                .bind(node)
                .bind(user_id)
                .fetch_one(&pool)
                .await?;
        let saved = patch(&panel, &cookie, node, settings).await?;
        assert_eq!(saved["settings"], created["settings"], "{kind}");
        assert_eq!(stored(&pool, node).await?, before, "{kind}");
        let after: (Uuid, String) =
            sqlx::query_as("SELECT uuid,credential FROM accesses WHERE node_id=$1 AND user_id=$2")
                .bind(node)
                .bind(user_id)
                .fetch_one(&pool)
                .await?;
        assert_eq!(identity, after, "{kind}");
        for key in ["password", "psk"] {
            assert!(saved["protocol_config"].get(key).is_none());
        }
        assert!(
            saved["settings"]["hysteria2"]
                .get("obfs_password")
                .is_none()
        );
        nodes.push((kind, node));
    }
    panel.publish_now().await?;
    sqlx::query("UPDATE server_module_status SET applied_rev=target_rev,healthy=TRUE")
        .execute(&pool)
        .await?;
    let bundle: String = sqlx::query_scalar(
        "SELECT bundle FROM deployments WHERE server_id=$1 ORDER BY rev DESC LIMIT 1",
    )
    .bind(server)
    .fetch_one(&pool)
    .await?;
    let bundle: apsp_protocol::Bundle = serde_json::from_str(&bundle)?;
    let native: Value = serde_json::from_str(&bundle.files["config.json"])?;
    let url = format!(
        "{}?format=singbox",
        user["subscription_url"]
            .as_str()
            .context("subscription URL")?
    );
    let subscription: Value = panel
        .client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    for (kind, node) in nodes {
        let tag = format!("node-{node}");
        let inbound = native["inbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|value| value["tag"] == tag)
            .context("inbound")?;
        let outbound = subscription["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|value| value["tag"] == tag)
            .context("outbound")?;
        match kind {
            "hysteria2" => {
                assert_eq!(inbound["masquerade"]["content"], "TEST_ONLY decoy response");
                assert!(outbound.get("masquerade").is_none());
                assert_eq!(outbound["bbr_profile"], "conservative");
            }
            "tuic" => {
                assert_eq!(outbound["udp_over_stream"], true);
                assert!(outbound.get("udp_relay_mode").is_none());
                assert!(inbound.get("udp_over_stream").is_none());
            }
            "anytls" => {
                assert_eq!(inbound["padding_scheme"], json!(["stop=1", "0=30-30"]));
                assert!(outbound.get("padding_scheme").is_none());
                assert_eq!(outbound["min_idle_session"], 2);
            }
            "shadowsocks2022" => {
                assert_eq!(inbound["multiplex"]["enabled"], true);
                assert_eq!(outbound["multiplex"]["protocol"], "smux");
                assert_eq!(outbound["udp_over_tcp"]["enabled"], true);
                assert!(inbound.get("udp_over_tcp").is_none());
            }
            "snell-v6" => {
                assert_eq!(inbound["mode"], "unshaped");
                assert_eq!(outbound["mode"], "unshaped");
                assert_eq!(outbound["reuse"], true);
                assert!(inbound.get("reuse").is_none());
            }
            "naive" => {
                assert_eq!(inbound["tls"]["min_version"], "1.2");
                assert_eq!(inbound["tls"]["handshake_timeout"], "15s");
                assert!(outbound["tls"].get("min_version").is_none());
                assert!(outbound["tls"].get("max_version").is_none());
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

#[sqlx::test]
async fn reality_transport_edits_preserve_keys_grants_and_reset_to_tcp(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Transport").await?;
    panel.enable_plugin(&cookie, server).await?;
    let created = create(&panel, &cookie, server, "vless-reality", json!({})).await?;
    let node = id(&created)?;
    let user = id(&panel.create_user(&cookie, "Transport subscriber").await?)?;
    let grant = panel.grant(&cookie, user, node).await?;
    let before = stored(&pool, node).await?;
    for transport in [
        json!({"type":"ws","path":"/stream","host":"edge.example.com","max_early_data":1024,"early_data_header_name":"Sec-WebSocket-Protocol"}),
        json!({"type":"httpupgrade","path":"/upgrade","host":"edge.example.com"}),
        json!({"type":"grpc","service_name":"relay"}),
    ] {
        let current = patch(&panel,&cookie,node,json!({"reality":{"flow":"none","max_time_difference_seconds":60},"transport":transport})).await?;
        assert_eq!(current["settings"]["transport"], transport);
        let after = stored(&pool, node).await?;
        for key in ["private_key", "public_key", "short_id", "protocol_config"] {
            assert_eq!(after[key], before[key]);
        }
        let grants = call(
            &panel,
            &cookie,
            Method::GET,
            &format!("/users/{user}/accesses"),
            None,
        )
        .await?;
        assert_eq!(grants[0]["uuid"], grant["uuid"]);
    }
    let reset = patch(
        &panel,
        &cookie,
        node,
        json!({"transport":{"type":"tcp"},"reality":{"flow":"vision"}}),
    )
    .await?;
    assert_eq!(reset["settings"]["transport"], json!({"type":"tcp"}));
    assert!(reset["settings"]["reality"]["max_time_difference_seconds"].is_null());
    Ok(())
}

#[sqlx::test]
async fn invalid_options_roll_back_the_entire_node_edit(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Validation").await?;
    panel.enable_plugin(&cookie, server).await?;
    for (kind, invalid) in [
        (
            "vless-reality",
            json!({"transport":{"type":"ws","path":"/x"}}),
        ),
        ("vless-reality", json!({"tcp_keep_alive_seconds":0})),
        (
            "vless-reality",
            json!({"disable_tcp_keep_alive":true,"tcp_keep_alive_seconds":10}),
        ),
        (
            "anytls",
            json!({"tls_min_version":"1.3","tls_max_version":"1.2"}),
        ),
        ("anytls", json!({"anytls":{"padding_scheme":["0=30-30"]}})),
        (
            "anytls",
            json!({"anytls":{"padding_scheme":["stop=1","0=bad"]}}),
        ),
        ("hysteria2", json!({"tls_max_version":"1.2"})),
        (
            "hysteria2",
            json!({"hysteria2":{"masquerade":{"status_code":99,"content_type":"text/plain","content":"invalid"}}}),
        ),
        ("tuic", json!({"tcp_keep_alive_seconds":10})),
        ("snell-v6", json!({"anytls":{"min_idle_session":1}})),
        (
            "shadowsocks2022",
            json!({"shadowsocks":{"multiplex":{"enabled":true,"max_streams":2,"max_connections":2}}}),
        ),
    ] {
        let node = id(&create(&panel, &cookie, server, kind, json!({})).await?)?;
        let before = stored(&pool, node).await?;
        let response = panel
            .admin(
                Method::PATCH,
                &format!("{ROOT}/nodes/{node}"),
                &cookie,
                Some(json!({"name":"Should rollback","settings":invalid})),
            )
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{kind}: {}",
            response.text().await?
        );
        assert_eq!(stored(&pool, node).await?, before);
        let current = call(
            &panel,
            &cookie,
            Method::GET,
            &format!("/nodes/{node}"),
            None,
        )
        .await?;
        assert_eq!(current["name"], kind);
    }
    Ok(())
}

#[sqlx::test]
async fn referenced_node_views_explain_immutable_configuration_before_editing(
    pool: PgPool,
) -> Result<()> {
    let panel =
        TestPanel::start_with_public_url(pool.clone(), Some("https://panel.example.com")).await?;
    let cookie = panel.admin_cookie().await?;
    let entry_server = panel.create_server(&cookie, "Entry").await?;
    let hop_server = panel.create_server(&cookie, "Hop").await?;
    for server in [entry_server, hop_server] {
        panel.enable_plugin(&cookie, server).await?;
        sqlx::query("UPDATE servers SET capabilities='[\"singbox\",\"runtime:dependency-validation:v1\"]' WHERE id=$1").bind(server).execute(&pool).await?;
    }
    let hop_settings =
        json!({"reality":{"flow":"none"},"transport":{"type":"grpc","service_name":"private-hop"}});
    let hop = id(&create(
        &panel,
        &cookie,
        hop_server,
        "vless-reality",
        hop_settings.clone(),
    )
    .await?)?;
    let receipt = call(&panel,&cookie,Method::POST,"/chains/batch",Some(json!({"request_id":Uuid::new_v4(),"items":[{
        "name":"Frozen path","entry":{"mode":"new","server_id":entry_server,"public_host":"entry.example.com","sni":"www.example.com"},
        "hops":[{"kind":"managed","node_id":hop}]
    }]}))).await?;
    let chain = receipt["chain_ids"][0].as_i64().context("chain")?;
    let entry = receipt["entry_node_ids"][0].as_i64().context("entry")?;
    // Simulate an existing database row before optional settings were added.
    sqlx::query("UPDATE nodes SET settings='{}' WHERE id=$1")
        .bind(entry)
        .execute(&pool)
        .await?;
    let same_defaults = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/nodes/{entry}"),
        None,
    )
    .await?;
    sqlx::query("UPDATE servers SET dirty_at=NULL WHERE id=$1")
        .bind(entry_server)
        .execute(&pool)
        .await?;
    call(
        &panel,
        &cookie,
        Method::PATCH,
        &format!("/nodes/{entry}"),
        Some(json!({"settings":same_defaults["settings"]})),
    )
    .await?;
    let dirty: Option<i64> = sqlx::query_scalar("SELECT dirty_at FROM servers WHERE id=$1")
        .bind(entry_server)
        .fetch_one(&pool)
        .await?;
    assert_eq!(dirty, None);
    let snapshot: Value = sqlx::query_scalar(
        "SELECT path_json FROM singbox_chain_versions WHERE chain_id=$1 AND generation=1",
    )
    .bind(chain)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        snapshot["hops"][0]["endpoint"]["settings"]["transport"],
        hop_settings["transport"]
    );
    for node in [entry, hop] {
        let expected = json!([{"id":chain,"name":"Frozen path"}]);
        let current = call(
            &panel,
            &cookie,
            Method::GET,
            &format!("/nodes/{node}"),
            None,
        )
        .await?;
        assert_eq!(current["configuration_locked"], true);
        assert_eq!(current["referenced_chains"], expected);
        let edited = call(
            &panel,
            &cookie,
            Method::PATCH,
            &format!("/nodes/{node}"),
            Some(json!({"name":"Allowed rename"})),
        )
        .await?;
        assert_eq!(edited["configuration_locked"], true);
        assert_eq!(edited["referenced_chains"], expected);
        let denied = panel
            .admin(
                Method::PATCH,
                &format!("{ROOT}/nodes/{node}"),
                &cookie,
                Some(json!({"settings":{"tcp_keep_alive_seconds":25}})),
            )
            .await?;
        assert_eq!(denied.status(), StatusCode::CONFLICT);
    }
    let nodes = call(&panel, &cookie, Method::GET, "/nodes", None).await?;
    assert!(
        nodes
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["configuration_locked"] == true)
    );
    call(
        &panel,
        &cookie,
        Method::DELETE,
        &format!("/proxy-resources/chain/{chain}"),
        None,
    )
    .await?;
    let unlocked = call(&panel, &cookie, Method::GET, &format!("/nodes/{hop}"), None).await?;
    assert_eq!(unlocked["configuration_locked"], false);
    assert_eq!(unlocked["referenced_chains"], json!([]));
    patch(&panel, &cookie, hop, json!({"reality":{"flow":"none"}})).await?;
    Ok(())
}

#[sqlx::test]
async fn legacy_configuration_lock_uses_public_identity_and_releases_after_shared_cleanup(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let entry_server = panel.create_server(&cookie, "Legacy entry").await?;
    let exit_server = panel.create_server(&cookie, "Legacy exit").await?;
    let entry = id(&panel.create_node(&cookie, entry_server, "Entry").await?)?;
    let exit = id(&panel.create_node(&cookie, exit_server, "Exit").await?)?;
    let created = panel
        .import_legacy_chain(&cookie, "Legacy owner", entry, exit)
        .await?;
    let chain = id(&created)?;
    let expected = json!([{"id":chain,"name":"Legacy owner"}]);
    for node in [entry, exit] {
        let view = call(
            &panel,
            &cookie,
            Method::GET,
            &format!("/nodes/{node}"),
            None,
        )
        .await?;
        assert_eq!(view["configuration_locked"], true);
        assert_eq!(view["referenced_chains"], expected);
        let denied = panel
            .admin(
                Method::PATCH,
                &format!("{ROOT}/nodes/{node}"),
                &cookie,
                Some(json!({"settings":{"tcp_keep_alive_seconds":25}})),
            )
            .await?;
        assert_eq!(denied.status(), StatusCode::CONFLICT);
    }
    call(
        &panel,
        &cookie,
        Method::DELETE,
        &format!("/ordered-proxy-resources/chain/{chain}"),
        None,
    )
    .await?;
    let unlocked = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/nodes/{exit}"),
        None,
    )
    .await?;
    assert_eq!(unlocked["configuration_locked"], false);
    assert_eq!(unlocked["referenced_chains"], json!([]));
    patch(&panel, &cookie, exit, json!({"tcp_keep_alive_seconds":25})).await?;
    let versions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM singbox_ordered_chain_versions WHERE chain_id=$1")
            .bind(chain)
            .fetch_one(&pool)
            .await?;
    assert_eq!(versions, 1);
    Ok(())
}

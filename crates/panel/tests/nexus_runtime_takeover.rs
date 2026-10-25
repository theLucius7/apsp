#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Result, ensure};
use apsp_compiler::{Node, ProtocolConfig, SsMethod};
use apsp_panel::{control_center::credentials, publisher, usage};
use apsp_protocol::{UsageBatch, UsageRecord, now_timestamp};
use base64::{Engine, engine::general_purpose::STANDARD};
use business_support::{TestPanel, release_fixture};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const ROOT: &str = "/api/plugins/sing-box/nexus-nodes/runtime";
async fn post(
    panel: &TestPanel,
    cookie: &str,
    path: &str,
    value: Value,
) -> Result<reqwest::Response> {
    panel
        .admin(Method::POST, &format!("{ROOT}{path}"), cookie, Some(value))
        .await
}
async fn ok(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let value: Value = response.json().await?;
    ensure!(
        status.is_success(),
        "runtime request failed: {status}: {value}"
    );
    Ok(value)
}

async fn fixture(panel: &TestPanel) -> Result<(i64, Value, i64)> {
    let pool = &panel.state.pool;
    let server: i64 =
        sqlx::query_scalar("INSERT INTO servers(name) VALUES('TEST_ONLY host') RETURNING id")
            .fetch_one(pool)
            .await?;
    sqlx::query("INSERT INTO singbox_nexus_node_imports VALUES('fixture',repeat('a',64),repeat('b',64),1,1,1)").execute(pool).await?;
    let key = STANDARD.encode(b"0123456789abcdef");
    let node = Node {
        id: 43,
        name: "TEST_ONLY SS".into(),
        port: 23456,
        public_host: "node.example.invalid".into(),
        sni: String::new(),
        private_key: String::new(),
        public_key: String::new(),
        short_id: String::new(),
        users: vec![],
        enabled: true,
        settings: Default::default(),
        protocol_config: ProtocolConfig::Shadowsocks2022 {
            method: SsMethod::Aes128,
            password: key.clone(),
        },
    };
    let original = json!({"node":{"id":43,"name":node.name,"host":node.public_host,"type":"shadowsocks","protocol_settings":{"cipher":"2022-blake3-aes-128-gcm"},"rate_time_enable":0}});
    let credential =
        credentials::store_generated(pool, "TEST_ONLY node", "backup", original).await?;
    sqlx::query("INSERT INTO singbox_nexus_nodes(source_id,legacy_node_id,legacy_machine_id,legacy_host_id,server_id,credential_id,display) VALUES('fixture',43,30,15,$1,$2,$3)")
        .bind(server).bind(credential).bind(json!({"listen_port":"23456","rate":"2.5","groups":[{"id":1,"name":"Fixture"}]})).execute(pool).await?;
    let payload = json!({"source_id":"fixture","server_id":server,"nodes":[{"legacy_node_id":43,"preserved":{"node":node,"configuration":{"inbounds":[{"type":"shadowsocks","tag":"in","listen":"::","listen_port":23456,"method":"2022-blake3-aes-128-gcm","password":key,"users":[]}],"outbounds":[{"type":"direct","tag":"direct"}],"route":{"final":"direct"}}}}]});
    let import = Uuid::new_v4();
    sqlx::query("INSERT INTO singbox_nexus_imports VALUES($1,'fixture',repeat('a',64),'{}',1,1)")
        .bind(import)
        .execute(pool)
        .await?;
    let user:i64=sqlx::query_scalar("INSERT INTO users(name,subscription_token) VALUES('TEST_ONLY user','TEST_ONLY unchanged token') RETURNING id").fetch_one(pool).await?;
    sqlx::query("INSERT INTO singbox_nexus_identities(user_id,import_id,legacy_uuid,original_state,used_upload,used_download) VALUES($1,$2,$3,'active',123,456)").bind(user).bind(import).bind(Uuid::parse_str("12345678-1234-4234-8234-123456789abc")?).execute(pool).await?;
    let group:i64=sqlx::query_scalar("INSERT INTO singbox_package_groups(name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,duration_days) VALUES('TEST_ONLY package',1000000,1,0,0,'UTC',31) RETURNING id").fetch_one(pool).await?;
    let assignment:i64=sqlx::query_scalar("INSERT INTO singbox_package_assignments(user_id,request_id,package_group_id,package_name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,starts_at,expires_at) VALUES($1,$2,$3,'TEST_ONLY',1000000,1,0,0,'UTC',0,253402300799) RETURNING id").bind(user).bind(Uuid::new_v4()).bind(group).fetch_one(pool).await?;
    sqlx::query("INSERT INTO singbox_user_packages VALUES($1,$2)")
        .bind(user)
        .bind(assignment)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO singbox_nexus_plan_mappings VALUES($1,$2,1,1)")
        .bind(import)
        .bind(group)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO singbox_nexus_bridge(singleton,import_id,subscription_base,enabled,last_sync_at,activated_at) VALUES(TRUE,$1,'https://subscriptions.example.invalid/assets/',TRUE,$2,$2)").bind(import).bind(now_timestamp()).execute(pool).await?;
    Ok((server, payload, user))
}

/// What the compatibility bridge delivers each cycle: legacy device limits and
/// the addresses legacy nodes see; returns apsp-admitted addresses per node.
async fn bridge_devices(pool: &PgPool, limits: Value, legacy: Value) -> Result<Value> {
    let import: Uuid = sqlx::query_scalar("SELECT import_id FROM singbox_nexus_bridge")
        .fetch_one(pool)
        .await?;
    let sample =
        json!({"import_id":import,"observed_at":now_timestamp(),"limits":limits,"legacy":legacy});
    Ok(
        sqlx::query_scalar("SELECT singbox_nexus_device_exchange($1)")
            .bind(sample)
            .fetch_one(pool)
            .await?,
    )
}

#[sqlx::test(migrations = "./migrations")]
async fn preserved_runtime_requires_explicit_handoff_and_replays_usage_once(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, payload, user) = fixture(&panel).await?;
    let preview = ok(post(&panel, &cookie, "/preview", json!({"payload":payload})).await?).await?;
    assert_eq!(preview["conflicts"], json!([]));
    let input = json!({"payload":payload,"expected_digest":preview["digest"]});
    let (a, b) = tokio::join!(
        post(&panel, &cookie, "/prepare", input.clone()),
        post(&panel, &cookie, "/prepare", input)
    );
    ok(a?).await?;
    ok(b?).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM nodes")
            .fetch_one(&pool)
            .await?,
        1
    );
    sqlx::query("UPDATE servers SET dirty_at=0")
        .execute(&pool)
        .await?;
    publisher::publish_due(&panel.state).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM deployments")
            .fetch_one(&pool)
            .await?,
        0
    );
    assert!(
        sqlx::query("UPDATE nodes SET port=23457")
            .execute(&pool)
            .await
            .is_err()
    );
    let candidate = ok(post(
        &panel,
        &cookie,
        &format!("/{server}/candidate"),
        Value::Null,
    )
    .await?)
    .await?;
    let body = json!({"expected_digest":preview["digest"],"config_sha256":candidate["config_sha256"],"backup_sha256":"a".repeat(64),"stopped_at":now_timestamp(),"stopped_services":["TEST_ONLY-old.service"]});
    assert_eq!(
        post(
            &panel,
            &cookie,
            &format!("/{server}/activate"),
            body.clone()
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    // Synthetic signed runtime is confined to this temporary TestPanel.
    let binary = b"TEST_ONLY signed runtime";
    let archive = release_fixture::archive("sing-box", binary)?;
    release_fixture::write(
        &panel.state.config.data_dir,
        "sing-box",
        "1.14.2",
        "sing-box",
        &archive,
        binary,
        "tar.gz",
    )?;
    sqlx::query("UPDATE servers SET device_public_key='TEST_ONLY',last_seen=$2,static_info=$3,capabilities=$4 WHERE id=$1").bind(server).bind(now_timestamp())
        .bind(json!({"arch":"amd64","os":"linux","libc":"gnu","agent_version":"0.3.1"})).bind(json!(["singbox",apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY])).execute(&pool).await?;
    // Without device limits from the bridge the handoff waits.
    assert_eq!(
        post(
            &panel,
            &cookie,
            &format!("/{server}/activate"),
            body.clone()
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    bridge_devices(&pool, json!({}), json!({})).await?;
    let mut stale = body.clone();
    stale["config_sha256"] = json!("b".repeat(64));
    assert_eq!(
        post(&panel, &cookie, &format!("/{server}/activate"), stale)
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    ok(post(
        &panel,
        &cookie,
        &format!("/{server}/activate"),
        body.clone(),
    )
    .await?)
    .await?;
    // Establish membership before forcing the test debounce deadline.
    publisher::publish_due(&panel.state).await?;
    sqlx::query("UPDATE servers SET dirty_at=0")
        .execute(&pool)
        .await?;
    publisher::publish_due(&panel.state).await?;
    let bundle: String =
        sqlx::query_scalar("SELECT bundle FROM deployments ORDER BY rev DESC LIMIT 1")
            .fetch_one(&pool)
            .await?;
    let bundle: apsp_protocol::Bundle = serde_json::from_str(&bundle)?;
    let config: Value = serde_json::from_str(&bundle.files["config.json"])?;
    let native: i64 = sqlx::query_scalar("SELECT node_id FROM singbox_nexus_runtime_nodes")
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        config["inbounds"][0]["users"][0]["password"],
        STANDARD.encode(b"12345678-1234-42")
    );
    assert_eq!(
        config["inbounds"][0]["users"][0]["name"],
        format!("u{user}_n{native}")
    );
    let batch = UsageBatch {
        epoch: Uuid::new_v4(),
        seq: 1,
        period_start: now_timestamp() - 10,
        period_end: now_timestamp(),
        records: vec![UsageRecord {
            stat_name: format!("u{user}_n{native}"),
            uplink: 10,
            downlink: 20,
        }],
    };
    usage::ingest(&panel.state, server, batch.clone()).await?;
    usage::ingest(&panel.state, server, batch).await?;
    let delivery: (i64, String, String) = sqlx::query_as(
        "SELECT COUNT(*),MAX(uplink)::text,MAX(rate)::text FROM singbox_nexus_usage_deliveries",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(delivery, (1, "10".into(), "2.500000".into()));
    let token: String = sqlx::query_scalar("SELECT subscription_token FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(&pool)
        .await?;
    assert_eq!(token, "TEST_ONLY unchanged token");
    // Moving to a different permission group removes the listener without a
    // password rotation or an accidentally exposed SS single-user listener.
    sqlx::query("UPDATE singbox_nexus_plan_mappings SET legacy_group_id=2")
        .execute(&pool)
        .await?;
    publisher::publish_due(&panel.state).await?;
    sqlx::query("UPDATE servers SET dirty_at=0")
        .execute(&pool)
        .await?;
    publisher::publish_due(&panel.state).await?;
    let saved: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM deployments")
        .fetch_one(&pool)
        .await?;
    assert_eq!(saved, 2);
    ok(post(&panel, &cookie, &format!("/{server}/pause"), body).await?).await?;
    sqlx::query("UPDATE servers SET dirty_at=0")
        .execute(&pool)
        .await?;
    publisher::publish_due(&panel.state).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM deployments")
            .fetch_one(&pool)
            .await?,
        saved
    );
    let diff: String =
        sqlx::query_scalar("SELECT string_agg(request_diff::text,'') FROM management_audit")
            .fetch_one(&pool)
            .await?;
    assert!(!diff.contains(&STANDARD.encode(b"0123456789abcdef")));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn device_limits_need_session_control_and_are_shared_with_legacy_nodes(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, payload, user) = fixture(&panel).await?;
    let preview = ok(post(&panel, &cookie, "/preview", json!({"payload":payload})).await?).await?;
    ok(post(
        &panel,
        &cookie,
        "/prepare",
        json!({"payload":payload,"expected_digest":preview["digest"]}),
    )
    .await?)
    .await?;
    let binary = b"TEST_ONLY signed runtime";
    let archive = release_fixture::archive("sing-box", binary)?;
    release_fixture::write(
        &panel.state.config.data_dir,
        "sing-box",
        "1.14.2",
        "sing-box",
        &archive,
        binary,
        "tar.gz",
    )?;
    let online = |capabilities: Value| {
        sqlx::query("UPDATE servers SET device_public_key='TEST_ONLY',last_seen=$2,static_info=$3,capabilities=$4 WHERE id=$1")
            .bind(server).bind(now_timestamp())
            .bind(json!({"arch":"amd64","os":"linux","libc":"gnu","agent_version":"0.3.2"}))
            .bind(capabilities)
            .execute(&pool)
    };
    let basic = json!([
        "singbox",
        apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY
    ]);
    online(basic.clone()).await?;
    // The legacy panel limits this member to three devices; legacy nodes
    // currently see it on one address.
    let empty = bridge_devices(
        &pool,
        json!({user.to_string(): 3}),
        json!({user.to_string(): ["198.51.100.1"]}),
    )
    .await?;
    assert_eq!(empty["managed"], json!([]), "nothing is taken over yet");
    let switch = |config: &Value| json!({"expected_digest":preview["digest"],"config_sha256":config["config_sha256"],"backup_sha256":"a".repeat(64),"stopped_at":now_timestamp(),"stopped_services":["TEST_ONLY-old.service"]});
    let plain = ok(post(
        &panel,
        &cookie,
        &format!("/{server}/candidate"),
        Value::Null,
    )
    .await?)
    .await?;
    let refused = post(
        &panel,
        &cookie,
        &format!("/{server}/activate"),
        switch(&plain),
    )
    .await?;
    assert_eq!(
        refused.status(),
        StatusCode::CONFLICT,
        "limited members need session control"
    );
    let mut capable = basic.as_array().unwrap().clone();
    capable.push(json!(apsp_protocol::RUNTIME_ADMISSION_CAPABILITY));
    online(json!(capable)).await?;
    let controlled = ok(post(
        &panel,
        &cookie,
        &format!("/{server}/candidate"),
        Value::Null,
    )
    .await?)
    .await?;
    assert_ne!(controlled["config_sha256"], plain["config_sha256"]);
    let secret: String = sqlx::query_scalar(
        "SELECT session_secret FROM singbox_nexus_runtime_hosts WHERE server_id=$1",
    )
    .bind(server)
    .fetch_one(&pool)
    .await?;
    assert_eq!(secret.len(), 64);
    assert_eq!(
        ok(post(
            &panel,
            &cookie,
            &format!("/{server}/candidate"),
            Value::Null
        )
        .await?)
        .await?["config_sha256"],
        controlled["config_sha256"],
        "the session secret is stable, so the candidate digest is too"
    );
    ok(post(
        &panel,
        &cookie,
        &format!("/{server}/activate"),
        switch(&controlled),
    )
    .await?)
    .await?;
    publisher::publish_due(&panel.state).await?;
    sqlx::query("UPDATE servers SET dirty_at=0")
        .execute(&pool)
        .await?;
    publisher::publish_due(&panel.state).await?;
    let bundle: String =
        sqlx::query_scalar("SELECT bundle FROM deployments ORDER BY rev DESC LIMIT 1")
            .fetch_one(&pool)
            .await?;
    let bundle: apsp_protocol::Bundle = serde_json::from_str(&bundle)?;
    let config: Value = serde_json::from_str(&bundle.files["config.json"])?;
    assert_eq!(config["log"]["level"], "info");
    assert_eq!(
        config["experimental"]["clash_api"]["external_controller"],
        "127.0.0.1:18086"
    );
    assert_eq!(config["experimental"]["clash_api"]["secret"], secret);
    let native: i64 = sqlx::query_scalar("SELECT node_id FROM singbox_nexus_runtime_nodes")
        .fetch_one(&pool)
        .await?;
    let principal = format!("u{user}_n{native}");
    // The device reports one admitted address; names of other nodes are ignored.
    apsp_panel::plugins::singbox::ingest_runtime_sessions(
        &panel.state,
        server,
        apsp_protocol::RuntimeSessionReport {
            module: "singbox".into(),
            observed_at: now_timestamp(),
            principals: [
                (principal.clone(), vec!["203.0.113.5".parse()?]),
                (
                    format!("u{user}_n{}", native + 1000),
                    vec!["203.0.113.6".parse()?],
                ),
            ]
            .into(),
        },
    )
    .await?;
    let stored: Vec<String> =
        sqlx::query_scalar("SELECT address FROM singbox_runtime_session_addresses")
            .fetch_all(&pool)
            .await?;
    assert_eq!(stored, vec!["203.0.113.5".to_string()]);
    // The bridge publishes apsp-admitted addresses under the legacy node id.
    let shared = bridge_devices(
        &pool,
        json!({user.to_string(): 3}),
        json!({user.to_string(): ["198.51.100.1"]}),
    )
    .await?;
    assert_eq!(shared["managed"][0]["legacy_node_id"], 43);
    assert_eq!(
        shared["managed"][0]["users"],
        json!({user.to_string():["203.0.113.5"]})
    );
    // No certified runtime yet: the legacy node status must not be refreshed.
    assert_eq!(shared["managed"][0]["healthy"], false);
    sqlx::query(
        "UPDATE server_module_status SET healthy=TRUE,applied_rev=target_rev WHERE server_id=$1",
    )
    .bind(server)
    .execute(&pool)
    .await?;
    sqlx::query("UPDATE servers SET last_seen=$2 WHERE id=$1")
        .bind(server)
        .bind(now_timestamp())
        .execute(&pool)
        .await?;
    let healthy = bridge_devices(
        &pool,
        json!({user.to_string(): 3}),
        json!({user.to_string(): ["198.51.100.1"]}),
    )
    .await?;
    assert_eq!(healthy["managed"][0]["healthy"], true);
    // The device's policy counts the legacy address but not its own sessions.
    let policy = apsp_panel::plugins::singbox::nexus_admission_policy(&panel.state, server)
        .await?
        .expect("limited member has a policy");
    assert_eq!(policy.groups.len(), 1);
    assert_eq!(policy.groups[0].principals, vec![principal]);
    assert_eq!(policy.groups[0].limit, 3);
    assert_eq!(
        policy.groups[0].remote,
        vec!["198.51.100.1".parse::<std::net::IpAddr>()?]
    );
    // Removing the last limit must send a replacement, not leave the old
    // policy in the Agent forever because there are no groups left to send.
    bridge_devices(&pool, json!({user.to_string(): 0}), json!({})).await?;
    let cleared = apsp_panel::plugins::singbox::nexus_admission_policy(&panel.state, server)
        .await?
        .expect("empty policy explicitly clears the previous limit");
    assert!(cleared.valid());
    assert!(cleared.groups.is_empty());
    // Malformed bridge samples are refused outright.
    for (limits, legacy) in [
        (json!({user.to_string(): 2000}), json!({})),
        (json!({}), json!({user.to_string(): ["198.51.100.0/24"]})),
        (json!({"0": 3}), json!({})),
    ] {
        assert!(bridge_devices(&pool, limits, legacy).await.is_err());
    }
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn expired_device_addresses_are_purged_without_new_reports(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let (server, _, user) = fixture(&panel).await?;
    let old = now_timestamp() - 301;
    sqlx::query("INSERT INTO singbox_runtime_session_addresses(server_id,stat_name,user_id,node_id,address,observed_at) VALUES($1,'u1_n1',$2,1,'192.0.2.1',$3)")
        .bind(server).bind(user).bind(old).execute(&pool).await?;
    sqlx::query("INSERT INTO singbox_nexus_device_state(user_id,device_limit,legacy_addresses,observed_at) VALUES($1,3,'[\"192.0.2.2\"]',$2)")
        .bind(user).bind(old).execute(&pool).await?;
    // No active runtime or session reports: the regular publisher cycle must
    // still enforce the retention limit on both kinds of address evidence.
    let publisher = tokio::spawn(publisher::run(panel.state.clone()));
    let expired = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let count: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM singbox_runtime_session_addresses)+(SELECT COUNT(*) FROM singbox_nexus_device_state WHERE legacy_addresses<>'[]'::jsonb)")
                .fetch_one(&pool).await?;
            if count == 0 { return Ok::<_, sqlx::Error>(()); }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }).await;
    publisher.abort();
    let _ = publisher.await;
    expired??;
    let limit: i32 =
        sqlx::query_scalar("SELECT device_limit FROM singbox_nexus_device_state WHERE user_id=$1")
            .bind(user)
            .fetch_one(&pool)
            .await?;
    assert_eq!(limit, 3, "expiry removes evidence, not configured limits");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn unactivated_preparation_can_be_discarded_and_generic_grants_are_refused(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, payload, _) = fixture(&panel).await?;
    let preview = ok(post(&panel, &cookie, "/preview", json!({"payload":payload})).await?).await?;
    let digest = preview["digest"].clone();
    ok(post(
        &panel,
        &cookie,
        "/prepare",
        json!({"payload":payload,"expected_digest":digest}),
    )
    .await?)
    .await?;
    let native: i64 = sqlx::query_scalar("SELECT node_id FROM singbox_nexus_runtime_nodes")
        .fetch_one(&pool)
        .await?;
    // Policy groups would mint new UUIDs; access stays on the legacy groups.
    let policy = panel
        .admin(
            Method::POST,
            "/api/plugins/sing-box/policy-groups",
            &cookie,
            Some(json!({"name":"TEST_ONLY policy","node_ids":[native],"chain_ids":[]})),
        )
        .await?;
    assert_eq!(policy.status(), StatusCode::CONFLICT);
    assert_eq!(
        post(
            &panel,
            &cookie,
            &format!("/{server}/discard"),
            json!({"expected_digest":"c".repeat(64)})
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    let discarded = ok(post(
        &panel,
        &cookie,
        &format!("/{server}/discard"),
        json!({"expected_digest":digest}),
    )
    .await?)
    .await?;
    assert_eq!(discarded["discarded_nodes"], 1);
    let remaining: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM nodes),(SELECT COUNT(*) FROM singbox_nexus_runtime_hosts),(SELECT COUNT(*) FROM credential_entries WHERE name LIKE 'Nexus runtime %' AND enabled)")
        .fetch_one(&pool).await?;
    assert_eq!(remaining, (0, 0, 0));
    // The same reviewed snapshot can be prepared again after the discard.
    let again = ok(post(
        &panel,
        &cookie,
        "/prepare",
        json!({"payload":payload,"expected_digest":digest}),
    )
    .await?)
    .await?;
    assert_eq!(again["state"], "prepared");
    // Once a host has published, its history stays and only pause applies.
    sqlx::query("UPDATE singbox_nexus_runtime_hosts SET activated_at=1")
        .execute(&pool)
        .await?;
    assert_eq!(
        post(
            &panel,
            &cookie,
            &format!("/{server}/discard"),
            json!({"expected_digest":digest})
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    Ok(())
}

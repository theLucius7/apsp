#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Result, ensure};
use apsp_access::usage;
use apsp_compiler::{Node, ProtocolConfig, SsMethod};
use apsp_governance::control_center::credentials;
use apsp_panel::publisher;
use apsp_protocol::{UsageBatch, UsageRecord, now_timestamp};
use base64::{Engine, engine::general_purpose::STANDARD};
use business_support::{TestPanel, release_fixture};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const ROOT: &str = "/api/traffic/nexus-nodes/runtime";
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
    // The member is in the policy group standing for original group 1 (ADR 0115).
    sqlx::query("INSERT INTO singbox_user_policies(user_id,group_id) VALUES($1,singbox_legacy_policy_group(1))")
        .bind(user)
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
    apsp_traffic::publisher::publish_due(&panel.state).await?;
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
        "1.14.4-apsp.1",
        "sing-box",
        &archive,
        binary,
        "tar.gz",
    )?;
    sqlx::query("UPDATE servers SET device_public_key='TEST_ONLY',last_seen=$2,static_info=$3,capabilities=$4 WHERE id=$1").bind(server).bind(now_timestamp())
        .bind(json!({"arch":"amd64","os":"linux","libc":"gnu","agent_version":"0.3.4"})).bind(json!(["singbox",apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY])).execute(&pool).await?;
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
    // An old Agent cannot accept the patched runtime, so the handoff waits.
    sqlx::query(
        "UPDATE servers SET static_info=static_info||'{\"agent_version\":\"0.3.3\"}' WHERE id=$1",
    )
    .bind(server)
    .execute(&pool)
    .await?;
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
    sqlx::query(
        "UPDATE servers SET static_info=static_info||'{\"agent_version\":\"0.3.4\"}' WHERE id=$1",
    )
    .bind(server)
    .execute(&pool)
    .await?;
    // Another host cut over recently: pacing holds this one back until the
    // operator explicitly overrides it.
    let neighbour: i64 =
        sqlx::query_scalar("INSERT INTO servers(name) VALUES('TEST_ONLY neighbour') RETURNING id")
            .fetch_one(&pool)
            .await?;
    let credential =
        credentials::store_generated(&pool, "TEST_ONLY neighbour", "backup", json!({"node":{}}))
            .await?;
    sqlx::query("INSERT INTO singbox_nexus_runtime_hosts(server_id,source_id,credential_id,digest,state,created_at,last_activated_at) VALUES($1,'fixture',$2,repeat('c',64),'active',1,$3)")
        .bind(neighbour)
        .bind(credential)
        .bind(now_timestamp() - 3600)
        .execute(&pool)
        .await?;
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
    let mut paced = body.clone();
    paced["ignore_stagger"] = json!(true);
    // A fresh blocked measurement holds the cutover. A stale one says nothing.
    sqlx::query("UPDATE singbox_nexus_nodes SET display=display||'{\"protocol\":\"shadowsocks\",\"public_host\":\"blocked.example.invalid\",\"public_port\":\"443\"}' WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO singbox_reachability_targets(host,port,state,checked_at,seen_at) VALUES('blocked.example.invalid',443,'ip_blocked',$1,$1)")
        .bind(now_timestamp())
        .execute(&pool)
        .await?;
    assert_eq!(
        post(
            &panel,
            &cookie,
            &format!("/{server}/activate"),
            paced.clone()
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    let mut forced = paced.clone();
    forced["ignore_reachability"] = json!(true);
    ok(post(&panel, &cookie, &format!("/{server}/activate"), forced).await?).await?;
    sqlx::query("UPDATE singbox_reachability_targets SET checked_at=1")
        .execute(&pool)
        .await?;
    ok(post(&panel, &cookie, &format!("/{server}/activate"), paced).await?).await?;
    let recorded: Option<i64> = sqlx::query_scalar(
        "SELECT last_activated_at FROM singbox_nexus_runtime_hosts WHERE server_id=$1",
    )
    .bind(server)
    .fetch_one(&pool)
    .await?;
    assert!(recorded.is_some());
    sqlx::query("DELETE FROM singbox_nexus_runtime_hosts WHERE server_id=$1")
        .bind(neighbour)
        .execute(&pool)
        .await?;
    // The catalog can follow a moved host without touching the preserved runtime.
    let inventory: i64 = sqlx::query_scalar(
        "SELECT inventory_id FROM singbox_nexus_runtime_nodes WHERE server_id=$1",
    )
    .bind(server)
    .fetch_one(&pool)
    .await?;
    let endpoint = format!("/api/traffic/nexus-nodes/{inventory}/public-endpoint");
    let moved = json!({"public_host":"moved.example.invalid","public_port":443});
    let response = panel
        .admin(Method::PATCH, &endpoint, &cookie, Some(moved))
        .await?;
    assert_eq!(ok(response).await?["public_port"], "443");
    let (display, public_host, settings): (Value, String, Value) = sqlx::query_as(
        "SELECT d.display,n.public_host,n.settings FROM singbox_nexus_nodes d JOIN singbox_nexus_runtime_nodes r ON r.inventory_id=d.id JOIN nodes n ON n.id=r.node_id WHERE d.id=$1",
    )
    .bind(inventory)
    .fetch_one(&pool)
    .await?;
    assert_eq!(display["public_host"], "moved.example.invalid");
    assert_eq!(display["public_port"], "443");
    assert_eq!(public_host, "moved.example.invalid");
    // Client links use the node's own public port; the listener stays on 23456.
    assert_eq!(settings["public_port"], 443);
    let listen: i32 = sqlx::query_scalar("SELECT n.port FROM nodes n JOIN singbox_nexus_runtime_nodes r ON r.node_id=n.id WHERE r.inventory_id=$1")
        .bind(inventory).fetch_one(&pool).await?;
    assert_eq!(listen, 23456);
    for invalid in [
        json!({"public_host":"https://bad","public_port":443}),
        json!({"public_host":"ok.example.invalid","public_port":0}),
    ] {
        assert_eq!(
            panel
                .admin(Method::PATCH, &endpoint, &cookie, Some(invalid))
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    // A permission group added at the source after import reaches the runtime:
    // the line joins the policy group standing for it, created on first use.
    let groups = format!("/api/traffic/nexus-nodes/{inventory}/groups");
    let added = json!({"groups":[{"id":1,"name":"Fixture"},{"id":7,"name":"Added"}]});
    for invalid in [
        json!({"groups":[]}),
        json!({"groups":[{"id":1,"name":"Fixture"},{"id":1,"name":"Fixture"}]}),
        json!({"groups":[{"id":0,"name":"Fixture"}]}),
    ] {
        assert_eq!(
            panel
                .admin(Method::PATCH, &groups, &cookie, Some(invalid))
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    sqlx::query("UPDATE servers SET dirty_at=NULL WHERE id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    ok(panel
        .admin(Method::PATCH, &groups, &cookie, Some(added))
        .await?)
    .await?;
    let (recorded, display, dirty): (Vec<i64>, Value, Option<i64>) = sqlx::query_as(
        "SELECT r.legacy_group_ids,d.display,s.dirty_at FROM singbox_nexus_runtime_nodes r JOIN singbox_nexus_nodes d ON d.id=r.inventory_id JOIN servers s ON s.id=r.server_id WHERE r.inventory_id=$1",
    )
    .bind(inventory)
    .fetch_one(&pool)
    .await?;
    assert_eq!(recorded, vec![1, 7]);
    assert_eq!(display["groups"][1]["name"], "Added");
    assert!(dirty.is_some(), "an active host must republish its members");
    let held = "SELECT g.legacy_group_id FROM singbox_policy_nodes p JOIN singbox_policy_groups g ON g.id=p.group_id JOIN singbox_nexus_runtime_nodes r ON r.node_id=p.node_id WHERE r.inventory_id=$1 ORDER BY 1";
    let in_groups: Vec<i64> = sqlx::query_scalar(held)
        .bind(inventory)
        .fetch_all(&pool)
        .await?;
    assert_eq!(in_groups, vec![1, 7]);
    let named: String =
        sqlx::query_scalar("SELECT name FROM singbox_policy_groups WHERE legacy_group_id=7")
            .fetch_one(&pool)
            .await?;
    assert_eq!(named, "原权限组 · Added");
    ok(panel
        .admin(
            Method::PATCH,
            &groups,
            &cookie,
            Some(json!({"groups":[{"id":1,"name":"Fixture"}]})),
        )
        .await?)
    .await?;
    let in_groups: Vec<i64> = sqlx::query_scalar(held)
        .bind(inventory)
        .fetch_all(&pool)
        .await?;
    assert_eq!(
        in_groups,
        vec![1],
        "the line leaves only the original group's policy group"
    );
    // The generic write path stays closed once the bypass transaction ended.
    assert!(
        sqlx::query("UPDATE nodes SET public_host='x.example.invalid' WHERE server_id=$1")
            .bind(server)
            .execute(&pool)
            .await
            .is_err()
    );
    // Establish membership before forcing the test debounce deadline.
    apsp_traffic::publisher::publish_due(&panel.state).await?;
    sqlx::query("UPDATE servers SET dirty_at=0")
        .execute(&pool)
        .await?;
    apsp_traffic::publisher::publish_due(&panel.state).await?;
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
    // Raw taken-over traffic goes through the legacy multiplier once. The
    // entitlement must not add those same records on top of the legacy meter.
    let now = now_timestamp();
    let mut meter_tx = pool.begin().await?;
    sqlx::query("INSERT INTO singbox_nexus_metering(user_id,upload,download,last_reset_at,next_reset_at,observed_at) VALUES($1,0,0,$2,$3,$4)")
        .bind(user).bind(now-86400).bind(now+86400).bind(now).execute(&mut *meter_tx).await?;
    for (delivered, metered, expected) in [(false, 0_i64, "0"), (true, 75, "75")] {
        sqlx::query("UPDATE singbox_nexus_metering SET upload=$2 WHERE user_id=$1")
            .bind(user)
            .bind(metered)
            .execute(&mut *meter_tx)
            .await?;
        if delivered {
            sqlx::query("UPDATE singbox_nexus_usage_deliveries SET delivered_at=$1")
                .bind(now)
                .execute(&mut *meter_tx)
                .await?;
        }
        let used: String =
            sqlx::query_scalar("SELECT used_bytes FROM singbox_entitlements($1) WHERE user_id=$2")
                .bind(now)
                .bind(user)
                .fetch_one(&mut *meter_tx)
                .await?;
        assert_eq!(used, expected);
    }
    // Ordinary native-node traffic has no delivery queue entry and still counts.
    let direct_server: i64 = sqlx::query_scalar(
        "INSERT INTO servers(name) VALUES('TEST_ONLY direct host') RETURNING id",
    )
    .fetch_one(&mut *meter_tx)
    .await?;
    let direct: i64 = sqlx::query_scalar("INSERT INTO nodes(server_id,name,port,public_host,sni,private_key,public_key,short_id) VALUES($1,'TEST_ONLY native',23457,'native.example.invalid','','','','') RETURNING id")
        .bind(direct_server).fetch_one(&mut *meter_tx).await?;
    let epoch = Uuid::new_v4();
    sqlx::query("INSERT INTO usage_batches(server_id,epoch,seq,payload_hash) VALUES($1,$2,1,repeat('a',64))")
        .bind(direct_server).bind(epoch).execute(&mut *meter_tx).await?;
    sqlx::query("INSERT INTO usage_records(server_id,epoch,seq,stat_name,user_id,node_id,uplink,downlink,period_start,period_end) VALUES($1,$2,1,$3,$4,$5,3,4,$6,$7)")
        .bind(direct_server).bind(epoch).bind(format!("u{user}_n{direct}")).bind(user).bind(direct).bind(now-10).bind(now).execute(&mut *meter_tx).await?;
    let used: String =
        sqlx::query_scalar("SELECT used_bytes FROM singbox_entitlements($1) WHERE user_id=$2")
            .bind(now)
            .bind(user)
            .fetch_one(&mut *meter_tx)
            .await?;
    assert_eq!(used, "82");
    meter_tx.rollback().await?;

    let token: String = sqlx::query_scalar("SELECT subscription_token FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(&pool)
        .await?;
    assert_eq!(token, "TEST_ONLY unchanged token");
    // Moving to a policy group without the line removes the listener without a
    // password rotation or an accidentally exposed SS single-user listener.
    sqlx::query(
        "UPDATE singbox_user_policies SET group_id=singbox_legacy_policy_group(2) WHERE user_id=$1",
    )
    .bind(user)
    .execute(&pool)
    .await?;
    apsp_traffic::publisher::publish_due(&panel.state).await?;
    sqlx::query("UPDATE servers SET dirty_at=0")
        .execute(&pool)
        .await?;
    apsp_traffic::publisher::publish_due(&panel.state).await?;
    let saved: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM deployments")
        .fetch_one(&pool)
        .await?;
    assert_eq!(saved, 2);
    ok(post(&panel, &cookie, &format!("/{server}/pause"), body).await?).await?;
    sqlx::query("UPDATE servers SET dirty_at=0")
        .execute(&pool)
        .await?;
    apsp_traffic::publisher::publish_due(&panel.state).await?;
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
        "1.14.4-apsp.1",
        "sing-box",
        &archive,
        binary,
        "tar.gz",
    )?;
    let online = |capabilities: Value| {
        sqlx::query("UPDATE servers SET device_public_key='TEST_ONLY',last_seen=$2,static_info=$3,capabilities=$4 WHERE id=$1")
            .bind(server).bind(now_timestamp())
            .bind(json!({"arch":"amd64","os":"linux","libc":"gnu","agent_version":"0.3.4"}))
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
    apsp_traffic::publisher::publish_due(&panel.state).await?;
    sqlx::query("UPDATE servers SET dirty_at=0")
        .execute(&pool)
        .await?;
    apsp_traffic::publisher::publish_due(&panel.state).await?;
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
    apsp_access::nexus::sessions::ingest(
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
    let policy = apsp_access::nexus::sessions::admission_policy(&panel.state, server)
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
    let cleared = apsp_access::nexus::sessions::admission_policy(&panel.state, server)
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
async fn unactivated_preparation_can_be_discarded_and_policy_groups_keep_identities(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, payload, user) = fixture(&panel).await?;
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
    // The line already joined the policy group of its original group.
    let original: i64 = sqlx::query_scalar("SELECT g.legacy_group_id FROM singbox_policy_nodes p JOIN singbox_policy_groups g ON g.id=p.group_id WHERE p.node_id=$1")
        .bind(native).fetch_one(&pool).await?;
    assert_eq!(original, 1);
    // Any policy group may hold a migrated line; it never mints a new UUID.
    let policy = ok(panel
        .admin(
            Method::POST,
            "/api/access/policy-groups",
            &cookie,
            Some(json!({"name":"TEST_ONLY policy","node_ids":[native],"chain_ids":[]})),
        )
        .await?)
    .await?;
    assert_eq!(policy["migrated_node_ids"], json!([native]));
    assert_eq!(policy["legacy_group_id"], Value::Null);
    let group = policy["id"].as_i64().expect("group id");
    ok(panel
        .admin(
            Method::PUT,
            &format!("/api/access/users/{user}/policy-groups"),
            &cookie,
            Some(json!({"group_ids":[group]})),
        )
        .await?)
    .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM accesses")
            .fetch_one(&pool)
            .await?,
        0
    );
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

/// Listen ports in the newest published configuration of a host.
async fn published_ports(pool: &PgPool, server: i64) -> Result<Vec<i64>> {
    let bundle: String = sqlx::query_scalar(
        "SELECT bundle FROM deployments WHERE server_id=$1 ORDER BY rev DESC LIMIT 1",
    )
    .bind(server)
    .fetch_one(pool)
    .await?;
    let bundle: apsp_protocol::Bundle = serde_json::from_str(&bundle)?;
    let config: Value = serde_json::from_str(&bundle.files["config.json"])?;
    let mut ports: Vec<i64> = config["inbounds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|inbound| inbound["listen_port"].as_i64())
        .collect();
    ports.sort_unstable();
    Ok(ports)
}

#[sqlx::test(migrations = "./migrations")]
async fn retired_line_leaves_the_running_host_and_the_bridge(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, mut payload, _user) = fixture(&panel).await?;
    // A second line on the same host, as on a relay host with several exits.
    let mut second = payload["nodes"][0].clone();
    second["legacy_node_id"] = json!(44);
    second["preserved"]["node"]["id"] = json!(44);
    second["preserved"]["node"]["name"] = json!("TEST_ONLY SS second");
    second["preserved"]["node"]["port"] = json!(23457);
    second["preserved"]["configuration"]["inbounds"][0]["listen_port"] = json!(23457);
    payload["nodes"].as_array_mut().unwrap().push(second);
    let credential = credentials::store_generated(
        &pool,
        "TEST_ONLY second node",
        "backup",
        json!({"node":{"id":44,"name":"TEST_ONLY SS second","host":"node.example.invalid","type":"shadowsocks","protocol_settings":{"cipher":"2022-blake3-aes-128-gcm"},"rate_time_enable":0}}),
    )
    .await?;
    sqlx::query("INSERT INTO singbox_nexus_nodes(source_id,legacy_node_id,legacy_machine_id,legacy_host_id,server_id,credential_id,display) VALUES('fixture',44,31,15,$1,$2,$3)")
        .bind(server).bind(credential).bind(json!({"name":"TEST_ONLY second","listen_port":"23457","rate":"1","groups":[{"id":1,"name":"Fixture"}]})).execute(&pool).await?;
    sqlx::query("UPDATE singbox_nexus_node_imports SET node_count=2")
        .execute(&pool)
        .await?;
    let preview = ok(post(&panel, &cookie, "/preview", json!({"payload":payload})).await?).await?;
    assert_eq!(preview["conflicts"], json!([]));
    ok(post(
        &panel,
        &cookie,
        "/prepare",
        json!({"payload":payload,"expected_digest":preview["digest"]}),
    )
    .await?)
    .await?;
    // The handoff itself is covered above; here the host simply runs.
    sqlx::query("UPDATE singbox_nexus_runtime_hosts SET state='active',activated_at=1,last_activated_at=1 WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE server_plugins SET enabled=TRUE WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    apsp_traffic::publisher::publish_due(&panel.state).await?;
    sqlx::query("UPDATE servers SET dirty_at=0")
        .execute(&pool)
        .await?;
    apsp_traffic::publisher::publish_due(&panel.state).await?;
    assert_eq!(published_ports(&pool, server).await?, vec![23456, 23457]);
    let managed = bridge_devices(&pool, json!({}), json!({})).await?;
    assert_eq!(managed["managed"].as_array().map(Vec::len), Some(2));

    let inventory = |legacy: i64| {
        sqlx::query_scalar::<_, i64>("SELECT id FROM singbox_nexus_nodes WHERE legacy_node_id=$1")
            .bind(legacy)
            .fetch_one(&pool)
    };
    let (first, retired) = (inventory(43).await?, inventory(44).await?);
    let path = |id: i64| format!("/api/traffic/nexus-nodes/{id}/retire");
    let native: i64 =
        sqlx::query_scalar("SELECT node_id FROM singbox_nexus_runtime_nodes WHERE inventory_id=$1")
            .bind(retired)
            .fetch_one(&pool)
            .await?;
    // A mistyped id must not remove the other line.
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &path(retired),
                &cookie,
                Some(json!({"legacy_node_id":43}))
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    sqlx::query("UPDATE servers SET dirty_at=NULL WHERE id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    let done = ok(panel
        .admin(
            Method::POST,
            &path(retired),
            &cookie,
            Some(json!({"legacy_node_id":44})),
        )
        .await?)
    .await?;
    assert_eq!(done["republish"], true);
    assert_eq!(done["runtime_node_id"], native);
    let state: (i64, i64, bool, Option<i64>, i32, bool, Option<i64>) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM singbox_nexus_runtime_nodes),(SELECT COUNT(*) FROM singbox_nexus_nodes),(SELECT enabled FROM nodes WHERE id=$1),(SELECT deleted_at FROM nodes WHERE id=$1),(SELECT node_count FROM singbox_nexus_node_imports),(SELECT enabled FROM credential_entries WHERE id=$2),(SELECT dirty_at FROM servers WHERE id=$3)")
        .bind(native).bind(credential).bind(server).fetch_one(&pool).await?;
    assert_eq!(
        (state.0, state.1, state.2, state.4, state.5),
        (1, 1, false, 1, false)
    );
    assert!(state.3.is_some(), "the native row is retired, not deleted");
    assert!(state.6.is_some(), "the running host must be republished");
    // The generic write path stays closed after the bypass.
    assert!(
        sqlx::query("UPDATE nodes SET port=23458 WHERE server_id=$1")
            .bind(server)
            .execute(&pool)
            .await
            .is_err()
    );
    // The snapshot still lists the line; publication leaves it out. The
    // membership change restarts the debounce, as on the first publication.
    apsp_traffic::publisher::publish_due(&panel.state).await?;
    sqlx::query("UPDATE servers SET dirty_at=0")
        .execute(&pool)
        .await?;
    apsp_traffic::publisher::publish_due(&panel.state).await?;
    assert_eq!(published_ports(&pool, server).await?, vec![23456]);
    let managed = bridge_devices(&pool, json!({}), json!({})).await?;
    assert_eq!(managed["managed"][0]["legacy_node_id"], 43);
    assert_eq!(managed["managed"].as_array().map(Vec::len), Some(1));
    // The last line of a host is not retired; the whole host is paused instead.
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &path(first),
                &cookie,
                Some(json!({"legacy_node_id":43}))
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &path(retired),
                &cookie,
                Some(json!({"legacy_node_id":44}))
            )
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    let audit: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM management_audit WHERE action='nexus.nodes.retire'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(audit, 1);
    Ok(())
}

/// A plain VLESS listener behind a tunnel (ADR 0109): archived with the legacy
/// row's own transport, previewed and prepared like any preserved node.
async fn tunnel_fixture(panel: &TestPanel, legacy: i64, archived: Value) -> Result<Value> {
    let pool = &panel.state.pool;
    let server: i64 = sqlx::query_scalar(
        "INSERT INTO servers(name) VALUES('TEST_ONLY tunnel host') RETURNING id",
    )
    .fetch_one(pool)
    .await?;
    sqlx::query("INSERT INTO singbox_nexus_node_imports VALUES('fixture',repeat('a',64),repeat('b',64),1,1,1) ON CONFLICT DO NOTHING").execute(pool).await?;
    let mut node = Node {
        id: legacy,
        name: format!("TEST_ONLY tunnel {legacy}"),
        port: 21443,
        public_host: "tunnel.example.invalid".into(),
        sni: "tunnel.example.invalid".into(),
        private_key: String::new(),
        public_key: String::new(),
        short_id: String::new(),
        users: vec![],
        enabled: true,
        settings: Default::default(),
        protocol_config: ProtocolConfig::VlessTunnel,
    };
    node.settings.listen = "127.0.0.1".into();
    node.settings.public_port = Some(21443);
    node.settings.transport = serde_json::from_value(json!({"type":"ws","path":"/TEST_ONLY"}))?;
    let original = json!({"node":{"id":legacy,"name":node.name,"host":node.public_host,"type":"vless","protocol_settings":archived,"rate_time_enable":0}});
    let credential =
        credentials::store_generated(pool, "TEST_ONLY tunnel", "backup", original).await?;
    sqlx::query("INSERT INTO singbox_nexus_nodes(source_id,legacy_node_id,legacy_machine_id,legacy_host_id,server_id,credential_id,display) VALUES('fixture',$4,40,20,$1,$2,$3)")
        .bind(server).bind(credential).bind(json!({"listen_port":"21443","rate":"1","groups":[{"id":1,"name":"Fixture"}]})).bind(legacy).execute(pool).await?;
    let configuration = json!({"inbounds":[{"type":"vless","tag":"vless-in","listen":"127.0.0.1","listen_port":21443,"multiplex":{"enabled":true},
        "transport":{"type":"ws","path":"/TEST_ONLY"},"users":[{"name":"placeholder","uuid":"00000000-0000-0000-0000-000000000000"}]}],
        "outbounds":[{"type":"direct","tag":"direct"},{"type":"block","tag":"block"}],"route":{"final":"direct"}});
    Ok(
        json!({"source_id":"fixture","server_id":server,"nodes":[{"legacy_node_id":legacy,"preserved":{"node":node,"configuration":configuration}}]}),
    )
}

#[sqlx::test]
async fn tunnel_origins_are_prepared_only_when_the_archive_matches(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let matching = json!({"tls":0,"network":"ws","network_settings":{"path":"/TEST_ONLY"}});
    let payload = tunnel_fixture(&panel, 66, matching).await?;
    let preview = ok(post(&panel, &cookie, "/preview", json!({"payload":payload})).await?).await?;
    assert_eq!(preview["conflicts"], json!([]));
    let prepared = ok(post(
        &panel,
        &cookie,
        "/prepare",
        json!({"payload":payload,"expected_digest":preview["digest"]}),
    )
    .await?)
    .await?;
    assert_eq!(prepared["state"], "prepared");
    let (protocol, listen): (String, String) = sqlx::query_as(
        "SELECT protocol,settings->>'listen' FROM nodes WHERE id=(SELECT node_id FROM singbox_nexus_runtime_nodes)",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        (protocol.as_str(), listen.as_str()),
        ("vless-tunnel", "127.0.0.1")
    );
    // Listener TLS, another transport or path, or another hostname in the
    // archive means the snapshot no longer describes that legacy node.
    for (legacy, archived) in [
        (
            67,
            json!({"tls":1,"network":"ws","network_settings":{"path":"/TEST_ONLY"}}),
        ),
        (
            68,
            json!({"tls":0,"network":"httpupgrade","network_settings":{"path":"/TEST_ONLY"}}),
        ),
        (
            69,
            json!({"tls":0,"network":"ws","network_settings":{"path":"/other"}}),
        ),
    ] {
        let payload = tunnel_fixture(&panel, legacy, archived.clone()).await?;
        let preview =
            ok(post(&panel, &cookie, "/preview", json!({"payload":payload})).await?).await?;
        assert!(
            preview["conflicts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c.as_str().unwrap().contains(&legacy.to_string())),
            "{archived}: {preview}"
        );
    }
    Ok(())
}

/// A tunnel origin imported onto an existing host: archive, inventory row and
/// the snapshot entry the exporter would produce for it.
async fn added_tunnel(panel: &TestPanel, server: i64, legacy: i64) -> Result<Value> {
    let pool = &panel.state.pool;
    let mut node = Node {
        id: legacy,
        name: format!("TEST_ONLY origin {legacy}"),
        port: 21000 + legacy as u16,
        public_host: "origin.example.invalid".into(),
        sni: "origin.example.invalid".into(),
        private_key: String::new(),
        public_key: String::new(),
        short_id: String::new(),
        users: vec![],
        enabled: true,
        settings: Default::default(),
        protocol_config: ProtocolConfig::VlessTunnel,
    };
    node.settings.listen = "127.0.0.1".into();
    node.settings.transport = serde_json::from_value(json!({"type":"ws","path":"/TEST_ONLY"}))?;
    let original = json!({"node":{"id":legacy,"name":node.name,"host":node.public_host,"type":"vless",
        "protocol_settings":{"tls":0,"network":"ws","network_settings":{"path":"/TEST_ONLY"}},"rate_time_enable":0}});
    let credential =
        credentials::store_generated(pool, "TEST_ONLY origin", "backup", original).await?;
    sqlx::query("INSERT INTO singbox_nexus_nodes(source_id,legacy_node_id,legacy_machine_id,legacy_host_id,server_id,credential_id,display) VALUES('fixture',$4,31,15,$1,$2,$3)")
        .bind(server).bind(credential).bind(json!({"listen_port":node.port.to_string(),"rate":"1","groups":[{"id":1,"name":"Fixture"}]})).bind(legacy).execute(pool).await?;
    let configuration = json!({"inbounds":[{"type":"vless","tag":"vless-in","listen":"127.0.0.1","listen_port":node.port,
        "transport":{"type":"ws","path":"/TEST_ONLY"},"users":[{"name":"placeholder","uuid":"00000000-0000-0000-0000-000000000000"}]}],
        "outbounds":[{"type":"direct","tag":"direct"}],"route":{"final":"direct"}});
    Ok(json!({"legacy_node_id":legacy,"preserved":{"node":node,"configuration":configuration}}))
}

#[sqlx::test]
async fn lines_are_added_to_a_prepared_or_running_host(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, payload, _) = fixture(&panel).await?;
    let preview = ok(post(&panel, &cookie, "/preview", json!({"payload":payload})).await?).await?;
    let digest = preview["digest"].as_str().unwrap().to_owned();
    ok(post(
        &panel,
        &cookie,
        "/prepare",
        json!({"payload":payload,"expected_digest":digest}),
    )
    .await?)
    .await?;
    let entry = added_tunnel(&panel, server, 66).await?;
    let extend_path = format!("/{server}/extend");
    let preview_path = format!("/{server}/extend/preview");
    let extend = |body: Value| post(&panel, &cookie, &extend_path, body);
    let preview_extension = |body: Value| post(&panel, &cookie, &preview_path, body);
    let counts = || async {
        sqlx::query_as::<_, (i64, i64)>("SELECT (SELECT COUNT(*) FROM nodes),(SELECT COUNT(*) FROM singbox_nexus_runtime_nodes)")
            .fetch_one(&pool).await
    };
    // The stored snapshot is pinned by its digest.
    assert_eq!(
        preview_extension(json!({"expected_digest":"c".repeat(64),"nodes":[entry]}))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let before = counts().await?;
    let report =
        ok(preview_extension(json!({"expected_digest":digest,"nodes":[entry]})).await?).await?;
    assert_eq!(report["conflicts"], json!([]));
    assert_eq!(report["added"], json!([66]));
    assert_eq!(report["node_count"], 2);
    let config: Value = serde_json::from_str(
        serde_json::from_str::<Value>(report["candidate"]["payload"]["bundle"].as_str().unwrap())?
            ["files"]["config.json"]
            .as_str()
            .unwrap(),
    )?;
    assert!(config["inbounds"].is_array());
    // The preview compiles in a transaction that is discarded.
    assert_eq!(counts().await?, before);
    let extended = report["digest"].as_str().unwrap().to_owned();
    let stopped = now_timestamp();
    // Committing needs the stopped legacy service and the previewed digest.
    assert_eq!(
        extend(json!({"expected_digest":digest,"extended_digest":extended,"nodes":[entry]}))
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        extend(json!({"expected_digest":digest,"extended_digest":"d".repeat(64),"nodes":[entry],"stopped_services":["xboard-node-cf.service"],"stopped_at":stopped})).await?.status(),
        StatusCode::CONFLICT
    );
    let previous: Uuid = sqlx::query_scalar(
        "SELECT credential_id FROM singbox_nexus_runtime_hosts WHERE server_id=$1",
    )
    .bind(server)
    .fetch_one(&pool)
    .await?;
    let done = ok(extend(json!({"expected_digest":digest,"extended_digest":extended,"nodes":[entry],"stopped_services":["xboard-node-cf.service"],"stopped_at":stopped})).await?).await?;
    assert_eq!(
        (done["republish"].clone(), done["digest"].as_str()),
        (json!(false), Some(extended.as_str()))
    );
    assert_eq!(counts().await?, (before.0 + 1, before.1 + 1));
    let (stored, protocol): (String, String) = sqlx::query_as("SELECT h.digest,n.protocol FROM singbox_nexus_runtime_hosts h JOIN singbox_nexus_runtime_nodes r ON r.server_id=h.server_id JOIN singbox_nexus_nodes i ON i.id=r.inventory_id AND i.legacy_node_id=66 JOIN nodes n ON n.id=r.node_id WHERE h.server_id=$1")
        .bind(server).fetch_one(&pool).await?;
    assert_eq!(
        (stored.as_str(), protocol.as_str()),
        (extended.as_str(), "vless-tunnel")
    );
    let retired: bool =
        sqlx::query_scalar("SELECT NOT enabled FROM credential_entries WHERE id=$1")
            .bind(previous)
            .fetch_one(&pool)
            .await?;
    assert!(retired);
    // The same line cannot be added twice; a line not imported to this host is refused.
    let again =
        ok(preview_extension(json!({"expected_digest":extended,"nodes":[entry]})).await?).await?;
    assert!(
        again["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c.as_str().unwrap().contains("66"))
    );
    let mut stray = entry.clone();
    stray["legacy_node_id"] = json!(77);
    stray["preserved"]["node"]["id"] = json!(77);
    stray["preserved"]["node"]["port"] = json!(21077);
    stray["preserved"]["configuration"]["inbounds"][0]["listen_port"] = json!(21077);
    let missing =
        ok(preview_extension(json!({"expected_digest":extended,"nodes":[stray]})).await?).await?;
    assert!(
        missing["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c.as_str().unwrap().contains("77"))
    );
    // A running host is published again with the added line.
    sqlx::query("UPDATE singbox_nexus_runtime_hosts SET state='active',activated_at=1,last_activated_at=1 WHERE server_id=$1")
        .bind(server).execute(&pool).await?;
    sqlx::query("UPDATE servers SET dirty_at=NULL WHERE id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    let second = added_tunnel(&panel, server, 67).await?;
    let report =
        ok(preview_extension(json!({"expected_digest":extended,"nodes":[second]})).await?).await?;
    assert_eq!(report["conflicts"], json!([]));
    let done = ok(extend(json!({"expected_digest":extended,"extended_digest":report["digest"],"nodes":[second],"stopped_services":["xboard-node-cf.service"],"stopped_at":now_timestamp()})).await?).await?;
    assert_eq!(done["republish"], true);
    let dirty: Option<i64> = sqlx::query_scalar("SELECT dirty_at FROM servers WHERE id=$1")
        .bind(server)
        .fetch_one(&pool)
        .await?;
    assert!(dirty.is_some());
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM management_audit WHERE action='nexus.runtime.extend'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(audited, 2);
    Ok(())
}

#[sqlx::test]
async fn a_line_configuration_is_revised_in_place(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, payload, _) = fixture(&panel).await?;
    let preview = ok(post(&panel, &cookie, "/preview", json!({"payload":payload})).await?).await?;
    let digest = preview["digest"].as_str().unwrap().to_owned();
    ok(post(
        &panel,
        &cookie,
        "/prepare",
        json!({"payload":payload,"expected_digest":digest}),
    )
    .await?)
    .await?;
    let preview_path = format!("/{server}/extend/preview");
    let extend_path = format!("/{server}/extend");
    // The legacy listener later gained reuse_addr; only the configuration changes.
    let mut configuration = payload["nodes"][0]["preserved"]["configuration"].clone();
    configuration["inbounds"][0]["reuse_addr"] = json!(true);
    let revision = json!({"expected_digest":digest,"configurations":[{"legacy_node_id":43,"configuration":configuration}]});
    let nodes_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes")
        .fetch_one(&pool)
        .await?;
    let report = ok(post(&panel, &cookie, &preview_path, revision.clone()).await?).await?;
    assert_eq!(report["conflicts"], json!([]));
    assert_eq!(
        (report["added"].clone(), report["revised"].clone()),
        (json!([]), json!([43]))
    );
    let mut commit = revision.clone();
    commit["extended_digest"] = report["digest"].clone();
    // A revision adds no listener, so no stopped service is required.
    let done = ok(post(&panel, &cookie, &extend_path, commit).await?).await?;
    assert_eq!(done["revised"], json!([43]));
    let nodes_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes")
        .fetch_one(&pool)
        .await?;
    assert_eq!(nodes_before, nodes_after);
    let candidate =
        ok(post(&panel, &cookie, &format!("/{server}/candidate"), json!({})).await?).await?;
    let config: Value = serde_json::from_str(
        serde_json::from_str::<Value>(candidate["payload"]["bundle"].as_str().unwrap())?["files"]["config.json"].as_str().unwrap(),
    )?;
    assert!(
        config["inbounds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["reuse_addr"] == true)
            || config["inbounds"].as_array().unwrap().is_empty()
    );
    // The revision is stored: the next revision starts from the new digest.
    let stored: String =
        sqlx::query_scalar("SELECT digest FROM singbox_nexus_runtime_hosts WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?;
    assert_eq!(Some(stored.as_str()), report["digest"].as_str());
    // A line outside the snapshot cannot be revised; an identity change is refused.
    let stray = json!({"expected_digest":stored,"configurations":[{"legacy_node_id":99,"configuration":configuration}]});
    let refused = ok(post(&panel, &cookie, &preview_path, stray).await?).await?;
    assert!(
        refused["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c.as_str().unwrap().contains("99"))
    );
    let mut moved = configuration.clone();
    moved["inbounds"][0]["listen_port"] = json!(23999);
    let identity = json!({"expected_digest":stored,"configurations":[{"legacy_node_id":43,"configuration":moved}]});
    assert_eq!(
        post(&panel, &cookie, &preview_path, identity)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn redeploy_repeats_a_running_hosts_bundle_as_a_higher_revision(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, payload, _user) = fixture(&panel).await?;
    let preview = ok(post(&panel, &cookie, "/preview", json!({"payload":payload})).await?).await?;
    ok(post(
        &panel,
        &cookie,
        "/prepare",
        json!({"payload":payload,"expected_digest":preview["digest"]}),
    )
    .await?)
    .await?;
    sqlx::query("UPDATE singbox_nexus_runtime_hosts SET state='active',activated_at=1,last_activated_at=1 WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE server_plugins SET enabled=TRUE WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    apsp_traffic::publisher::publish_due(&panel.state).await?;
    sqlx::query("UPDATE servers SET dirty_at=0")
        .execute(&pool)
        .await?;
    apsp_traffic::publisher::publish_due(&panel.state).await?;
    let revisions = || async {
        sqlx::query_as::<_, (i64, String)>(
            "SELECT rev,bundle_sha256 FROM deployments WHERE server_id=$1 ORDER BY rev",
        )
        .bind(server)
        .fetch_all(&pool)
        .await
    };
    let before = revisions().await?;
    let (current, digest) = before.last().cloned().expect("running host deployment");

    // A running migrated host takes the same administrator recovery as any other server.
    business_support::prove(&pool, &cookie).await?;
    let response = panel
        .admin(
            Method::POST,
            &format!("/api/traffic/servers/{server}/deployments/redeploy"),
            &cookie,
            None,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["revision"], current + 1);
    let after = revisions().await?;
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(after.last(), Some(&(current + 1, digest)));
    // Scheduled publication of the unchanged host keeps the repeated revision.
    sqlx::query("UPDATE servers SET dirty_at=0")
        .execute(&pool)
        .await?;
    apsp_traffic::publisher::publish_due(&panel.state).await?;
    assert_eq!(revisions().await?.len(), after.len());
    Ok(())
}

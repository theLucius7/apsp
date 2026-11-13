#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result, ensure};
use apsp_governance::{auth::hash_token, control_center::credentials};
use apsp_protocol::now_timestamp;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

const ROOT: &str = "/api/traffic/nexus-nodes";
const SECRET: &str = "fixture-private-node-material";

async fn request(
    panel: &TestPanel,
    cookie: &str,
    method: Method,
    suffix: &str,
    body: Value,
) -> Result<reqwest::Response> {
    let body = if suffix == "/preview" {
        json!({"payload":body})
    } else {
        body
    };
    Ok(panel
        .client
        .request(method, format!("{}{ROOT}{suffix}", panel.base))
        .header("Cookie", cookie)
        .header("Origin", &panel.state.config.public_url)
        .json(&body)
        .send()
        .await?)
}

async fn ok(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let body = response.text().await?;
    // Do not print imported fixtures if a request unexpectedly exposes a secret.
    ensure!(!body.contains(SECRET), "response exposed private material");
    ensure!(status.is_success(), "unexpected response: {status}: {body}");
    Ok(serde_json::from_str(&body)?)
}

async fn prove(panel: &TestPanel, cookie: &str) -> Result<()> {
    let hash = hash_token(cookie.split_once('=').context("cookie")?.1);
    sqlx::query("INSERT INTO administrator_reauth(session_hash,verified_at,expires_at) VALUES($1,$2,$3) ON CONFLICT(session_hash) DO UPDATE SET verified_at=EXCLUDED.verified_at,expires_at=EXCLUDED.expires_at").bind(hash).bind(now_timestamp()).bind(now_timestamp()+3600).execute(&panel.state.pool).await?;
    Ok(())
}

async fn fixture(pool: &PgPool) -> Result<Value> {
    let namespace = format!("{:x}", Sha256::digest(b"fixture"));
    let mut hosts = Vec::new();
    for id in 1..=2 {
        let server: i64 = sqlx::query_scalar(
            "INSERT INTO servers(name,asset_settings) VALUES('fixture host',$1) RETURNING id",
        )
        .bind(json!({"tags":[format!("nexus:{}:{id}",&namespace[..12])]}))
        .fetch_one(pool)
        .await?;
        hosts.push(json!({"legacy_id":id,"server_id":server}));
    }
    let base = json!({"id":31,"name":"Fixture VLESS","type":"vless","code":null,"parent_id":0,"machine_id":7,
        "host":"node.example.invalid","port":"443","server_port":8443,"rate":"2.5","show":1,"enabled":1,"sort":0,
        "tags":"[\"测试\"]","group_ids":"[\"1\"]","route_ids":"[2]","rate_time_enable":0,"rate_time_ranges":null,
        "transfer_enable":null,"custom_outbounds":[{"type":"socks","password":SECRET}],"custom_routes":[{"domain":"example.invalid","outbound":"fixture"}],
        "cert_config":{"key":SECRET},"protocol_settings":{"tls":2,"network":"tcp","reality_settings":{"short_id":"0123456789abcdef","public_key":"fixture-public","private_key":SECRET,"server_name":"example.invalid"},"unexpected":{"new_secret":SECRET}}});
    let mut hysteria = base.clone();
    hysteria["id"] = json!(42);
    hysteria["type"] = json!("hysteria");
    hysteria["name"] = json!("Fixture Hysteria2");
    hysteria["machine_id"] = json!(8);
    hysteria["port"] = json!("21000-21020");
    hysteria["server_port"] = json!(21000);
    hysteria["rate"] = json!(0);
    hysteria["show"] = json!(0);
    hysteria["protocol_settings"]=json!({"version":2,"hop_interval":30,"obfs":{"open":true,"password":SECRET},"tls":{"server_name":"example.invalid","ech":{"key":SECRET}}}).to_string().into();
    let mut shadowsocks = base.clone();
    shadowsocks["id"] = json!(43);
    shadowsocks["type"] = json!("shadowsocks");
    shadowsocks["machine_id"] = json!(9);
    shadowsocks["enabled"] = json!(0);
    shadowsocks["protocol_settings"] =
        json!({"cipher":"2022-blake3-aes-128-gcm","password":SECRET});
    Ok(
        json!({"format":"apsp.nexus.nodes.v1","source_id":"fixture","infrastructure_digest":"a".repeat(64),"exported_at":1,
        "hosts":hosts,"machines":[{"id":7,"host_id":1,"name":"vless instance","is_active":1},{"id":8,"host_id":1,"name":"hysteria instance","is_active":1},{"id":9,"host_id":2,"name":"shadowsocks instance","is_active":0}],
        "groups":[{"id":1,"name":"Fixture group"}],"routes":[{"id":2,"remarks":"Fixture route","action_value":SECRET}],"nodes":[base,hysteria,shadowsocks]}),
    )
}

async fn counts(pool: &PgPool) -> Result<(i64, i64, i64)> {
    Ok(sqlx::query_as("SELECT (SELECT COUNT(*) FROM singbox_nexus_node_imports),(SELECT COUNT(*) FROM singbox_nexus_nodes),(SELECT COUNT(*) FROM credential_entries)").fetch_one(pool).await?)
}

#[sqlx::test(migrations = "./migrations")]
async fn node_inventory_is_atomic_encrypted_idempotent_and_does_not_publish(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let snapshot = fixture(&pool).await?;
    let preview =
        ok(request(&panel, &cookie, Method::POST, "/preview", snapshot.clone()).await?).await?;
    assert_eq!(preview["node_count"], 3);
    assert_eq!(preview["conflicts"], json!([]));
    assert_eq!(counts(&pool).await?, (0, 0, 0));
    let input =
        json!({"payload":snapshot,"expected_digest":preview["digest"],"confirm_staged":true});
    // Login itself grants a recent proof. Expire it to exercise the guard.
    sqlx::query("UPDATE administrator_reauth SET expires_at=0")
        .execute(&pool)
        .await?;
    assert!(
        !request(&panel, &cookie, Method::POST, "/import", input.clone())
            .await?
            .status()
            .is_success()
    );
    prove(&panel, &cookie).await?;
    // Force failure after the first node and credential have been inserted.
    sqlx::raw_sql("CREATE FUNCTION reject_fixture_node() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.legacy_node_id=42 THEN RAISE EXCEPTION 'fixture write failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_fixture BEFORE INSERT ON singbox_nexus_nodes FOR EACH ROW EXECUTE FUNCTION reject_fixture_node();").execute(&pool).await?;
    assert_eq!(
        request(&panel, &cookie, Method::POST, "/import", input.clone())
            .await?
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(counts(&pool).await?, (0, 0, 0));
    sqlx::query("DROP TRIGGER reject_fixture ON singbox_nexus_nodes")
        .execute(&pool)
        .await?;
    let (first, second) = tokio::join!(
        request(&panel, &cookie, Method::POST, "/import", input.clone()),
        request(&panel, &cookie, Method::POST, "/import", input.clone())
    );
    let first = ok(first?).await?;
    let second = ok(second?).await?;
    assert_ne!(first["already_imported"], second["already_imported"]);
    assert_eq!(counts(&pool).await?, (1, 3, 3));
    let public = ok(request(&panel, &cookie, Method::GET, "", Value::Null).await?).await?;
    let nodes = public.as_array().context("nodes")?;
    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[0]["protocol_summary"]["short_id"], "0123456789abcdef");
    assert_eq!(nodes[0]["rate"], "2.5");
    assert_eq!(nodes[1]["rate"], "0");
    assert_eq!(nodes[1]["public_port"], "21000-21020");
    assert_eq!(nodes[1]["visible"], false);
    assert_eq!(nodes[0]["server_id"], nodes[1]["server_id"]);
    assert_ne!(nodes[0]["legacy_machine_id"], nodes[1]["legacy_machine_id"]);
    assert_eq!(nodes[2]["enabled"], false);
    for node in nodes {
        assert_eq!(node["state"], "pending_agent");
        assert!(node.get("credential_id").is_none());
    }
    let records:Vec<(i64,Uuid,Vec<u8>)>=sqlx::query_as("SELECT n.legacy_node_id,n.credential_id,c.ciphertext FROM singbox_nexus_nodes n JOIN credential_entries c ON c.id=n.credential_id ORDER BY n.legacy_node_id").fetch_all(&pool).await?;
    for (id, credential, ciphertext) in records {
        assert!(!String::from_utf8_lossy(&ciphertext).contains(SECRET));
        let raw =
            credentials::resolve_reference_pool(&pool, credential, "backup", "inventory-test")
                .await?;
        assert_eq!(
            raw["node"],
            snapshot["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["id"] == id)
                .unwrap()
                .clone()
        );
        assert_eq!(raw["routes"][0]["action_value"], SECRET);
    }
    let untouched:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT COUNT(*) FROM nodes),(SELECT COUNT(*) FROM server_plugins),(SELECT COUNT(*) FROM deployments),(SELECT COUNT(*) FROM servers WHERE dirty_at IS NOT NULL)").fetch_one(&pool).await?;
    assert_eq!(untouched, (0, 0, 0, 0));
    let audits: Value = sqlx::query_scalar(
        "SELECT COALESCE(jsonb_agg(to_jsonb(a)),'[]'::jsonb) FROM management_audit a",
    )
    .fetch_one(&pool)
    .await?;
    assert!(!audits.to_string().contains(SECRET));
    // An exact retry keeps the same ids; changed source data cannot overwrite it.
    ok(request(&panel, &cookie, Method::POST, "/import", input.clone()).await?).await?;
    assert_eq!(
        public,
        ok(request(&panel, &cookie, Method::GET, "", Value::Null).await?).await?
    );
    let mut changed = input.clone();
    changed["payload"]["nodes"][0]["rate"] = json!(9);
    assert_eq!(
        request(&panel, &cookie, Method::POST, "/import", changed.clone())
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let changed_preview = ok(request(
        &panel,
        &cookie,
        Method::POST,
        "/preview",
        changed["payload"].clone(),
    )
    .await?)
    .await?;
    assert!(!changed_preview["conflicts"].as_array().unwrap().is_empty());
    changed["expected_digest"] = changed_preview["digest"].clone();
    assert_eq!(
        request(&panel, &cookie, Method::POST, "/import", changed)
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(counts(&pool).await?, (1, 3, 3));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn node_inventory_rejects_untrusted_mappings_and_invalid_references(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let snapshot = fixture(&pool).await?;
    for suffix in ["", "/preview", "/import"] {
        let method = if suffix.is_empty() {
            Method::GET
        } else {
            Method::POST
        };
        let body = if suffix == "/import" {
            json!({"payload":snapshot,"expected_digest":"a".repeat(64),"confirm_staged":true})
        } else {
            snapshot.clone()
        };
        assert_eq!(
            request(&panel, "", method, suffix, body).await?.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let mut wrong = snapshot.clone();
    let one = wrong["hosts"][0]["server_id"].clone();
    wrong["hosts"][0]["server_id"] = wrong["hosts"][1]["server_id"].clone();
    wrong["hosts"][1]["server_id"] = one;
    let preview =
        ok(request(&panel, &cookie, Method::POST, "/preview", wrong.clone()).await?).await?;
    assert_eq!(preview["conflicts"].as_array().unwrap().len(), 2);
    prove(&panel, &cookie).await?;
    assert_eq!(
        request(
            &panel,
            &cookie,
            Method::POST,
            "/import",
            json!({"payload":wrong,"expected_digest":preview["digest"],"confirm_staged":true})
        )
        .await?
        .status(),
        StatusCode::CONFLICT
    );
    for (path, value) in [
        ("/nodes/1/id", json!(31)),
        ("/nodes/0/machine_id", json!(999)),
        ("/nodes/0/group_ids", json!([999])),
        ("/nodes/0/route_ids", json!([999])),
        ("/nodes/0/port", json!("21020-21000")),
        ("/machines/1/host_id", json!(999)),
    ] {
        let mut invalid = snapshot.clone();
        *invalid.pointer_mut(path).unwrap() = value;
        assert_eq!(
            request(&panel, &cookie, Method::POST, "/preview", invalid)
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(counts(&pool).await?, (0, 0, 0));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn append_adds_only_new_nodes_and_keeps_the_original_import(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let snapshot = fixture(&pool).await?;
    // A node the source gained after its first import, on an existing host.
    let mut addition = snapshot.clone();
    let mut node = snapshot["nodes"][0].clone();
    node["id"] = json!(44);
    node["name"] = json!("Fixture added later");
    node["server_port"] = json!(8444);
    addition["nodes"] = json!([node]);
    addition["hosts"] = json!([snapshot["hosts"][0]]);
    addition["machines"] = json!([snapshot["machines"][0]]);
    addition["infrastructure_digest"] = json!("b".repeat(64));
    let preview = |body: Value| {
        let (panel, cookie) = (&panel, &cookie);
        async move {
            ok(request(
                panel,
                cookie,
                Method::POST,
                "/append/preview",
                json!({"payload":body}),
            )
            .await?)
            .await
        }
    };
    // Nothing to append to before the source itself was imported.
    let early = preview(addition.clone()).await?;
    assert_eq!(early["conflicts"][0], "来源尚未完整导入，不能追加节点");
    let base =
        ok(request(&panel, &cookie, Method::POST, "/preview", snapshot.clone()).await?).await?;
    prove(&panel, &cookie).await?;
    ok(request(
        &panel,
        &cookie,
        Method::POST,
        "/import",
        json!({"payload":snapshot,"expected_digest":base["digest"],"confirm_staged":true}),
    )
    .await?)
    .await?;
    assert_eq!(counts(&pool).await?, (1, 3, 3));

    let planned = preview(addition.clone()).await?;
    assert_eq!(planned["conflicts"], json!([]));
    assert_eq!(planned["new_nodes"], json!([44]));
    assert_eq!(planned["already_imported"], false);
    let input =
        json!({"payload":addition,"expected_digest":planned["digest"],"confirm_staged":true});
    let stale = json!({"payload":addition,"expected_digest":"c".repeat(64),"confirm_staged":true});
    assert_eq!(
        request(&panel, &cookie, Method::POST, "/append", stale)
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let done = ok(request(&panel, &cookie, Method::POST, "/append", input.clone()).await?).await?;
    assert_eq!(done["new_nodes"], json!([44]));
    assert_eq!(counts(&pool).await?, (1, 4, 4));
    let (count, digest): (i32, String) = sqlx::query_as(
        "SELECT node_count,digest FROM singbox_nexus_node_imports WHERE source_id='fixture'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        (count, digest.as_str()),
        (4, base["digest"].as_str().unwrap())
    );
    let (server, display): (i64, Value) =
        sqlx::query_as("SELECT server_id,display FROM singbox_nexus_nodes WHERE legacy_node_id=44")
            .fetch_one(&pool)
            .await?;
    assert_eq!(server, snapshot["hosts"][0]["server_id"].as_i64().unwrap());
    assert_eq!(display["listen_port"], "8444");
    // A retried request changes nothing.
    let again = ok(request(&panel, &cookie, Method::POST, "/append", input).await?).await?;
    assert_eq!(again["already_imported"], true);
    assert_eq!(counts(&pool).await?, (1, 4, 4));
    // The original import stays idempotent after an append.
    let replay =
        ok(request(&panel, &cookie, Method::POST, "/preview", snapshot.clone()).await?).await?;
    assert_eq!(replay["already_imported"], true);
    assert_eq!(replay["conflicts"], json!([]));

    // Mixing an imported node with a new one is refused.
    let mut mixed = addition.clone();
    let mut extra = mixed["nodes"][0].clone();
    extra["id"] = json!(45);
    mixed["nodes"].as_array_mut().unwrap().push(extra);
    let refused = preview(mixed).await?;
    assert_eq!(
        refused["conflicts"][0],
        "追加内容与已导入节点部分重叠，请只提交新节点"
    );
    // An imported node cannot be moved to another host by an append.
    let mut moved = snapshot.clone();
    moved["nodes"] = json!([snapshot["nodes"][0]]);
    moved["hosts"] = json!([{"legacy_id":1,"server_id":snapshot["hosts"][1]["server_id"]}]);
    moved["machines"] = json!([snapshot["machines"][0]]);
    let moved = preview(moved).await?;
    assert!(
        moved["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "原节点 31 已导入且归属不同，不能追加")
    );
    // A host without the source tag cannot receive nodes.
    let untagged: i64 =
        sqlx::query_scalar("INSERT INTO servers(name) VALUES('untagged') RETURNING id")
            .fetch_one(&pool)
            .await?;
    let mut stray = addition.clone();
    stray["nodes"][0]["id"] = json!(46);
    stray["hosts"] = json!([{"legacy_id":1,"server_id":untagged}]);
    let stray = preview(stray).await?;
    assert_eq!(
        stray["conflicts"][0],
        "原主机 1 的来源标签与目标服务器映射不一致"
    );
    let audit: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM management_audit WHERE action='nexus.nodes.append'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(audit, 1);
    Ok(())
}

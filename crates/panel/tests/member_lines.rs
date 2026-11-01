#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result, ensure};
use apsp_panel::{
    auth::{hash_token, random_token},
    control_center::credentials,
};
use apsp_protocol::{now_timestamp, telemetry::now_millis};
use business_support::TestPanel;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// A public address used only as a fixture value; the member view must never echo it.
const ADDRESS: &str = "8.8.4.4";

async fn session(pool: &PgPool, user: i64) -> Result<(Uuid, String)> {
    let account = Uuid::new_v4();
    let token = random_token();
    sqlx::query("INSERT INTO passkey_accounts(id) VALUES($1)")
        .bind(account)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO singbox_portal_accounts(user_id,account_id) VALUES($1,$2)")
        .bind(user)
        .bind(account)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO singbox_portal_sessions(token_hash,account_id,expires_at,verified_at) VALUES($1,$2,$3,$4)").bind(hash_token(&token)).bind(account).bind(now_timestamp()+3600).bind(now_timestamp()).execute(pool).await?;
    Ok((account, format!("apsp_proxy_session={token}")))
}

async fn node(pool: &PgPool, server: i64, id: i64, display: Value) -> Result<()> {
    let credential = credentials::store_generated(
        pool,
        "TEST_ONLY node",
        "backup",
        json!({"node":{"id":id,"private_key":"TEST_ONLY_SECRET"}}),
    )
    .await?;
    sqlx::query("INSERT INTO singbox_nexus_nodes(source_id,legacy_node_id,legacy_machine_id,legacy_host_id,server_id,credential_id,display) VALUES('fixture',$1,$1,$1,$2,$3,$4)")
        .bind(id).bind(server).bind(credential).bind(display).execute(pool).await?;
    Ok(())
}

async fn status(
    pool: &PgPool,
    import: Uuid,
    observed: i64,
    nodes: Value,
) -> Result<Value, sqlx::Error> {
    sqlx::query_scalar("SELECT singbox_nexus_node_status_exchange($1)")
        .bind(json!({"import_id":import,"observed_at":observed,"nodes":nodes}))
        .fetch_one(pool)
        .await
}

async fn view(
    panel: &TestPanel,
    account: Uuid,
    cookie: Option<&str>,
) -> Result<(StatusCode, String)> {
    let mut request = panel.client.get(format!(
        "{}/api/plugins/sing-box/portal/{account}/lines",
        panel.base
    ));
    if let Some(cookie) = cookie {
        request = request.header("Cookie", cookie);
    }
    let response = request.send().await?;
    Ok((response.status(), response.text().await?))
}

#[sqlx::test(migrations = "./migrations")]
async fn members_see_lines_status_and_measured_network_without_private_details(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let now = now_timestamp();
    // One host with an apsp Agent, one still served only by the legacy panel.
    let agent: i64 = sqlx::query_scalar("INSERT INTO servers(name,last_seen,latest_metrics,static_info) VALUES('TEST_ONLY agent host',$1,$2,$3) RETURNING id")
        .bind(now).bind(json!({"cpu_percent":12.34,"memory_used":512,"memory_total":1024,"disk_used":1,"disk_total":4,"uptime_secs":3600}))
        .bind(json!({"ip_addresses":[ADDRESS,"10.0.0.2"]})).fetch_one(&pool).await?;
    let legacy: i64 = sqlx::query_scalar(
        "INSERT INTO servers(name) VALUES('TEST_ONLY legacy host') RETURNING id",
    )
    .fetch_one(&pool)
    .await?;
    sqlx::query("INSERT INTO singbox_nexus_node_imports VALUES('fixture',repeat('a',64),repeat('b',64),1,1,4)").execute(&pool).await?;
    let summary = json!({"network":"tcp","tls":2,"reality_sni":"sni.example.invalid","public_key":"TEST_ONLY_KEY","short_id":"ab"});
    node(&pool, agent, 41, json!({"name":"🇭🇰 香港 01","protocol":"vless","protocol_summary":summary,"public_host":"entry.example.invalid","rate":"1","enabled":true,"visible":true,"sort_order":1,"tags":["provider:Example"],"groups":[{"id":1,"name":"A"}]})).await?;
    node(&pool, agent, 42, json!({"name":"日本中转","protocol":"hysteria2","protocol_summary":{},"public_host":"entry.example.invalid","rate":"2","enabled":true,"visible":true,"sort_order":2,"tags":["relay","geo:JP","dynamic-residential"],"groups":[{"id":2,"name":"B"}]})).await?;
    node(&pool, legacy, 43, json!({"name":"隐藏线路","protocol":"vless","public_host":"hidden.example.invalid","enabled":true,"visible":false,"groups":[{"id":1}]})).await?;
    node(&pool, legacy, 44, json!({"name":"🇺🇸 美国 01","protocol":"trojan","protocol_summary":{"tls":1,"tls_sni":"sni.example.invalid"},"public_host":"us.example.invalid","rate":"0.5","enabled":true,"visible":true,"sort_order":3,"tags":[],"groups":[{"id":1,"name":"A"}]})).await?;

    let import = Uuid::new_v4();
    sqlx::query("INSERT INTO singbox_nexus_imports VALUES($1,'fixture',repeat('a',64),'{}',1,1)")
        .bind(import)
        .execute(&pool)
        .await?;
    let user: i64 = sqlx::query_scalar("INSERT INTO users(name,subscription_token) VALUES('TEST_ONLY member','TEST_ONLY token') RETURNING id").fetch_one(&pool).await?;
    let mut groups = Vec::new();
    for (name, legacy_group) in [("TEST_ONLY 标准", 1_i64), ("TEST_ONLY 高级", 2)] {
        let group: i64 = sqlx::query_scalar("INSERT INTO singbox_package_groups(name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,duration_days) VALUES($1,1000000,1,0,0,'UTC',31) RETURNING id").bind(name).fetch_one(&pool).await?;
        sqlx::query("INSERT INTO singbox_nexus_plan_mappings VALUES($1,$2,$3,$4)")
            .bind(import)
            .bind(group)
            .bind(legacy_group + 10)
            .bind(legacy_group)
            .execute(&pool)
            .await?;
        groups.push(group);
    }
    let assignment: i64 = sqlx::query_scalar("INSERT INTO singbox_package_assignments(user_id,request_id,package_group_id,package_name,monthly_bytes,reset_day,reset_hour,reset_minute,timezone,starts_at,expires_at) VALUES($1,$2,$3,'TEST_ONLY 标准',1000000,1,0,0,'UTC',0,253402300799) RETURNING id").bind(user).bind(Uuid::new_v4()).bind(groups[0]).fetch_one(&pool).await?;
    sqlx::query("INSERT INTO singbox_user_packages VALUES($1,$2)")
        .bind(user)
        .bind(assignment)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO singbox_nexus_bridge(singleton,import_id,subscription_base,enabled,last_sync_at,activated_at) VALUES(TRUE,$1,'https://subscriptions.example.invalid/assets/',TRUE,$2,$2)").bind(import).bind(now).execute(&pool).await?;

    // The bridge validates every legacy status before storing it.
    let entry = |id: i64| json!({"id":id,"online":true,"checked_at":now,"load":{"cpu":5,"memory":50.5,"disk":20},"latency":{"monitored":true,"updated_at":now,"carriers":{"telecom":{"latest":150,"average":160,"loss":0}}}});
    for invalid in [
        json!([{"id":44,"online":true,"load":{"address":ADDRESS}}]),
        json!([{"id":44,"online":true,"latency":{"carriers":{"other":{"latest":1}}}}]),
        json!([{"id":44,"online":true,"latency":{"carriers":{"telecom":{"target":ADDRESS}}}}]),
        json!([{"id":"x","online":true}]),
        json!([{"id":44,"online":"yes"}]),
    ] {
        ensure!(
            status(&pool, import, now, invalid).await.is_err(),
            "invalid status was stored"
        );
    }
    ensure!(
        status(&pool, import, now - 600, json!([entry(44)]))
            .await
            .is_err(),
        "expired status was stored"
    );
    ensure!(
        status(&pool, Uuid::new_v4(), now, json!([entry(44)]))
            .await
            .is_err(),
        "another import was accepted"
    );
    assert_eq!(
        status(&pool, import, now, json!([entry(44), entry(42), entry(99)])).await?["nodes"],
        3
    );
    assert_eq!(
        status(&pool, import, now, json!([entry(44), entry(42)])).await?["nodes"],
        2
    );
    let stored: Vec<i64> =
        sqlx::query_scalar("SELECT legacy_node_id FROM singbox_nexus_node_status ORDER BY 1")
            .fetch_all(&pool)
            .await?;
    assert_eq!(
        stored,
        [42, 44],
        "nodes the legacy panel no longer lists lose their status"
    );

    // Agent-measured carrier latency and IP quality for the Agent host.
    let probe = Uuid::new_v4();
    sqlx::query("INSERT INTO network_probes(id,server_id,spec) VALUES($1,$2,$3)")
        .bind(probe).bind(agent).bind(json!({"id":probe,"name":"TEST_ONLY","kind":"icmp","target":"192.0.2.1","port":null,"interval_secs":60,"carrier":"","enabled":true,"monitor":{"network":"unicom","region":"广东","address_family":"ipv4","authorization":null}}))
        .execute(&pool).await?;
    sqlx::query("INSERT INTO probe_results(id,server_id,probe_id,sampled_at,result,digest) VALUES($1,$2,$3,$4,$5,'TEST_ONLY')")
        .bind(Uuid::new_v4()).bind(agent).bind(probe).bind(now_millis() - 1000).bind(json!({"latency_ms":33.4,"loss_percent":0.0}))
        .execute(&pool).await?;
    let quality = json!({"ip":ADDRESS,"checked_at":now,"expires_at":now+3600,"status":"succeeded","databases":[{"database":"dbip-v2","label":"DB-IP","status":"succeeded","error":null,"last_success_at":now,"fields":[
        {"label":"ASN","value":64500,"kind":"asn"},{"label":"网络组织","value":"Example Network","kind":"text"},{"label":"用途类型","value":"isp","kind":"text"},
        {"label":"城市","value":"TEST_ONLY_CITY","kind":"text"},{"label":"纬度","value":22.3,"kind":"latitude"},{"label":"代理","value":false,"kind":"boolean"},{"label":"国家代码","value":"HK","kind":"country_code"}]}]});
    sqlx::query("INSERT INTO server_ip_quality(server_id,ip,provider,payload,checked_at,last_attempt_at,last_success_at,fresh_until) VALUES($1,$2,'check-place',$3,$4,$4,$4,$5)")
        .bind(agent).bind(ADDRESS).bind(quality).bind(now).bind(now + 3600).execute(&pool).await?;

    let (account, cookie) = session(&pool, user).await?;
    assert_eq!(
        view(&panel, account, None).await?.0,
        StatusCode::UNAUTHORIZED
    );
    let other: i64 = sqlx::query_scalar("INSERT INTO users(name,subscription_token) VALUES('TEST_ONLY other','TEST_ONLY other token') RETURNING id").fetch_one(&pool).await?;
    let (_, foreign) = session(&pool, other).await?;
    assert_eq!(
        view(&panel, account, Some(&foreign)).await?.0,
        StatusCode::UNAUTHORIZED,
        "a session only reads its own account"
    );

    let (code, text) = view(&panel, account, Some(&cookie)).await?;
    ensure!(code.is_success(), "{code}: {text}");
    for private in [
        ADDRESS,
        "10.0.0.2",
        "example.invalid",
        "TEST_ONLY_KEY",
        "TEST_ONLY_SECRET",
        "TEST_ONLY_CITY",
        "22.3",
        "192.0.2.1",
        "short_id",
    ] {
        ensure!(!text.contains(private), "member view leaked {private}");
    }
    let value: Value = serde_json::from_str(&text)?;
    assert_eq!(value["package"], "TEST_ONLY 标准");
    let lines = value["lines"].as_array().context("lines")?;
    let ids: Vec<_> = lines.iter().map(|line| line["id"].as_i64()).collect();
    assert_eq!(
        ids,
        [Some(41), Some(42), Some(44)],
        "hidden lines stay hidden"
    );

    let direct = &lines[0];
    assert_eq!(direct["topology"]["kind"], "direct");
    assert_eq!(direct["topology"]["region"], "HK");
    assert_eq!(direct["topology"]["provider"], "Example");
    assert_eq!(
        direct["protocol"],
        json!({"name":"vless","network":"tcp","security":"reality"})
    );
    assert_eq!(
        direct["status"]["state"], "unknown",
        "no status was reported for this legacy line"
    );
    assert_eq!(direct["access"], true);
    assert_eq!(
        direct["plans"],
        json!([{"name":"TEST_ONLY 标准","current":true}])
    );
    assert_eq!(direct["latency"]["source"], "apsp_probe");
    assert_eq!(direct["latency"]["carriers"]["unicom"]["latest"], 33.0);
    assert_eq!(direct["network_scope"], "entry");

    let relay = &lines[1];
    assert_eq!(relay["topology"]["kind"], "relay");
    assert_eq!(relay["topology"]["source"], "shared_ingress");
    assert_eq!(relay["topology"]["front_node_id"], 41);
    assert_eq!(relay["topology"]["inferred"], true);
    assert_eq!(relay["topology"]["residential"], true);
    assert_eq!(relay["topology"]["dynamic"], true);
    assert_eq!(relay["status"]["state"], "entry_online");
    assert_eq!(relay["access"], false);
    assert_eq!(
        relay["plans"],
        json!([{"name":"TEST_ONLY 高级","current":false}])
    );
    assert_eq!(relay["latency"]["referenced"], true);
    assert_eq!(relay["host"], direct["host"]);
    assert_eq!(relay["network_scope"], "egress_unverified");

    let us = &lines[2];
    assert_eq!(
        us["status"],
        json!({"state":"online","checked_at":now,"source":"legacy"})
    );
    assert_eq!(us["protocol"]["security"], "tls");
    assert_eq!(us["latency"]["source"], "legacy_monitor");
    assert_eq!(us["latency"]["carriers"]["telecom"]["latest"], 150);

    let hosts = value["hosts"].as_array().context("hosts")?;
    assert_eq!(hosts.len(), 2);
    let measured = hosts
        .iter()
        .find(|host| host["key"] == direct["host"])
        .context("agent host")?;
    assert_eq!(measured["source"], "apsp");
    assert_eq!(measured["online"], true);
    assert_eq!(
        measured["load"],
        json!({"cpu":12.3,"memory":50.0,"disk":25.0})
    );
    assert_eq!(measured["uptime_secs"], 3600);
    assert_eq!(measured["region"], "HK");
    let family = &measured["network"]["families"][0];
    assert_eq!(family["family"], "IPv4");
    assert_eq!(
        family["summary"],
        json!({"asn":64500,"country":"HK","organization":"Example Network"})
    );
    let labels: Vec<_> = family["datasets"][0]["fields"]
        .as_array()
        .context("fields")?
        .iter()
        .map(|field| field["label"].as_str())
        .collect();
    assert_eq!(
        labels,
        [
            Some("ASN"),
            Some("网络组织"),
            Some("用途类型"),
            Some("代理"),
            Some("国家代码")
        ],
        "cities and coordinates stay private"
    );
    let old = hosts
        .iter()
        .find(|host| host["key"] == us["host"])
        .context("legacy host")?;
    assert_eq!(old["source"], "legacy");
    assert_eq!(old["load"], json!({"cpu":5,"memory":50.5,"disk":20}));
    assert_eq!(
        old["network"],
        json!({"available":false,"reason":"no_agent"})
    );
    ensure!(
        hosts
            .iter()
            .all(|host| host["key"].as_str().is_some_and(|key| key.starts_with('h'))),
        "server ids stay internal"
    );

    // Stale legacy status lapses instead of claiming the node is online.
    sqlx::query("UPDATE singbox_nexus_node_status SET observed_at=observed_at-301")
        .execute(&pool)
        .await?;
    let value: Value = serde_json::from_str(&view(&panel, account, Some(&cookie)).await?.1)?;
    assert_eq!(value["lines"][2]["status"]["state"], "unknown");
    Ok(())
}

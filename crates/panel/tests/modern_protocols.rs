#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result};
use apsp_protocol::{ApplyResult, ApplyStatus};
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn protocol(kind: &str) -> Value {
    match kind {
        "hysteria2" | "tuic" | "anytls" | "naive" => {
            json!({"type": kind, "tls": {"mode":"acme", "email":"admin@example.com", "challenge":"http-01"}})
        }
        "shadowsocks2022" => json!({"type":kind,"method":"2022-blake3-aes-256-gcm"}),
        _ => json!({"type":kind}),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn new_protocol_credentials_snapshots_and_revocation(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Modern protocols").await?;
    panel.enable_plugin(&cookie, server).await?;
    let first = panel.create_user(&cookie, "First").await?;
    let second = panel.create_user(&cookie, "Second").await?;
    let first_id = id(&first)?;
    let second_id = id(&second)?;
    let mut nodes = Vec::new();
    for kind in [
        "hysteria2",
        "shadowsocks2022",
        "tuic",
        "anytls",
        "naive",
        "snell-v6",
    ] {
        let config = protocol(kind);
        let response = panel
            .admin(
                Method::POST,
                "/api/plugins/sing-box/nodes",
                &cookie,
                Some(json!({
                    "name": kind, "server_id": server, "public_host":"proxy.example.com",
                    "sni": if config.get("tls").is_some() { "proxy.example.com" } else { "" },
                    "protocol_config": config
                })),
            )
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "{}",
            response.text().await?
        );
        // Reload independently to check management responses do not expose node secrets.
        let views: Vec<Value> = panel
            .admin(Method::GET, "/api/plugins/sing-box/nodes", &cookie, None)
            .await?
            .json()
            .await?;
        let node = views.last().context("created node")?;
        assert_eq!(node["protocol"], kind);
        assert!(node["protocol_config"].get("password").is_none());
        assert!(node["protocol_config"].get("psk").is_none());
        let node_id = id(node)?;
        let grant = panel.grant(&cookie, first_id, node_id).await?;
        assert_eq!(grant, panel.grant(&cookie, first_id, node_id).await?);
        panel.grant(&cookie, second_id, node_id).await?;
        let rows =
            sqlx::query("SELECT credential,uuid FROM accesses WHERE node_id=$1 ORDER BY user_id")
                .bind(node_id)
                .fetch_all(&pool)
                .await?;
        assert_ne!(
            rows[0].get::<String, _>("credential"),
            rows[1].get::<String, _>("credential")
        );
        nodes.push(node_id);
    }
    panel.publish_now().await?;
    let snapshot: Value =
        sqlx::query_scalar("SELECT source_json FROM deployments WHERE server_id=$1 AND rev=1")
            .bind(server)
            .fetch_one(&pool)
            .await?;
    let model: Vec<apsp_compiler::Node> = serde_json::from_value(snapshot)?;
    assert_eq!(model.len(), 6);
    let sub = format!(
        "{}/sub/{}?format=singbox",
        panel.base,
        first["subscription_token"].as_str().context("token")?
    );
    assert_eq!(
        panel.client.get(&sub).send().await?.status(),
        StatusCode::CONFLICT
    );
    apsp_panel::agent_api::record_apply_result(
        &panel.state,
        server,
        ApplyResult {
            module: "singbox".into(),
            rev: 1,
            op_id: Uuid::new_v4(),
            status: ApplyStatus::Applied,
            healthy: true,
            error: None,
        },
    )
    .await?;
    let response = panel.client.get(&sub).send().await?.error_for_status()?;
    assert_eq!(response.headers()["cache-control"], "no-store");
    let client = response.text().await?;
    for node in &model {
        assert!(
            !client.contains(
                &node
                    .users
                    .iter()
                    .find(|access| access.user_id == second_id)
                    .unwrap()
                    .credential
            )
        );
        assert!(!client.contains("admin@example.com"));
    }
    assert_eq!(
        serde_json::from_str::<Value>(&client)?["outbounds"]
            .as_array()
            .unwrap()
            .len(),
        8
    );
    assert_eq!(
        panel
            .client
            .get(sub.replace("format=singbox", "format=links"))
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let node = nodes[5];
    panel
        .admin(
            Method::DELETE,
            &format!("/api/plugins/sing-box/users/{first_id}/accesses/{node}"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?;
    panel.grant(&cookie, first_id, node).await?;
    // Regrant must not publish old applied credentials before the new revision is healthy.
    let client: Value = panel
        .client
        .get(&sub)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(client["outbounds"].as_array().unwrap().len(), 7);
    assert!(!client.to_string().contains(&format!("node-{node}\"")));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn certificate_conflicts_and_protocol_changes_are_atomic(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Certificates").await?;
    panel.enable_plugin(&cookie, server).await?;
    let create = |port, config| json!({"name":"TLS node", "server_id":server, "public_host":"proxy.example.com", "sni":"proxy.example.com", "port":port, "protocol_config":config});
    let response = panel
        .admin(
            Method::POST,
            "/api/plugins/sing-box/nodes",
            &cookie,
            Some(create(20443, protocol("anytls"))),
        )
        .await?
        .error_for_status()?;
    let node = id(&response.json().await?)?;
    let mut conflict = protocol("tuic");
    conflict["tls"]["email"] = json!("other@example.com");
    for body in [
        create(20444, conflict),
        create(80, protocol("naive")),
        create(
            20444,
            json!({"type":"anytls","tls":{"mode":"manual","certificate":"invalid","key":"invalid"}}),
        ),
    ] {
        let response = panel
            .admin(
                Method::POST,
                "/api/plugins/sing-box/nodes",
                &cookie,
                Some(body),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let response = panel
        .admin(
            Method::PATCH,
            &format!("/api/plugins/sing-box/nodes/{node}"),
            &cookie,
            Some(json!({"protocol_config":{"type":"snell-v6"}, "sni":""})),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 1);
    let config: Value = sqlx::query_scalar("SELECT protocol_config FROM nodes WHERE id=$1")
        .bind(node)
        .fetch_one(&pool)
        .await?;
    assert_eq!(config["type"], "anytls");
    panel
        .admin(
            Method::POST,
            "/api/plugins/sing-box/nodes",
            &cookie,
            Some(create(20444, protocol("tuic"))),
        )
        .await?
        .error_for_status()?;
    let mut shared = protocol("anytls");
    shared["tls"]["challenge"] = json!("tls-alpn-01");
    shared["tls"]["email"] = json!("updated@example.com");
    panel
        .admin(
            Method::PATCH,
            &format!("/api/plugins/sing-box/nodes/{node}"),
            &cookie,
            Some(json!({"protocol_config":shared})),
        )
        .await?
        .error_for_status()?;
    let settings: Vec<Value> =
        sqlx::query_scalar("SELECT protocol_config->'tls' FROM nodes ORDER BY id")
            .fetch_all(&pool)
            .await?;
    assert_eq!(settings, vec![shared["tls"].clone(), shared["tls"].clone()]);
    // A conflict found after the shared update rolls back every node together.
    let mut invalid = shared.clone();
    invalid["tls"]["email"] = json!("rolled-back@example.com");
    let response = panel
        .admin(
            Method::PATCH,
            &format!("/api/plugins/sing-box/nodes/{node}"),
            &cookie,
            Some(json!({"port":443,"protocol_config":invalid})),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let after: Vec<Value> =
        sqlx::query_scalar("SELECT protocol_config->'tls' FROM nodes ORDER BY id")
            .fetch_all(&pool)
            .await?;
    assert_eq!(after, settings);
    Ok(())
}

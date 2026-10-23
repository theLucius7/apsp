use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use sqlx::Row;

async fn modern_node(panel: &TestPanel, cookie: &str, server: i64, kind: &str) -> Result<i64> {
    panel.enable_plugin(cookie, server).await?;
    let tls = matches!(kind, "hysteria2" | "tuic" | "anytls" | "naive");
    let config = if tls {
        json!({"type":kind,"tls":{"mode":"acme","email":"admin@example.com","challenge":"http-01"}})
    } else {
        json!({"type":kind})
    };
    id(&call(
        panel,
        cookie,
        Method::POST,
        "/nodes",
        Some(json!({
            "name":kind,"server_id":server,"public_host":"proxy.example.com",
            "sni":if tls { "proxy.example.com" } else { "" },"protocol_config":config
        })),
    )
    .await?)
}

#[sqlx::test(migrations = "./migrations")]
async fn group_credentials_and_quota_cover_all_modern_protocols_without_losing_managed_tls(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Mixed protocols").await?;
    let first = panel.create_user(&cookie, "First").await?;
    let second = panel.create_user(&cookie, "Second").await?;
    let uid = id(&first)?;
    let other = id(&second)?;
    let mut nodes = vec![];
    for kind in [
        "hysteria2",
        "shadowsocks2022",
        "tuic",
        "anytls",
        "naive",
        "snell-v6",
    ] {
        nodes.push(modern_node(&panel, &cookie, server, kind).await?);
    }
    let group = create_policy(&panel, &cookie, &nodes, &[]).await?;
    set_policies(&panel, &cookie, uid, &[group]).await?;
    set_policies(&panel, &cookie, other, &[group]).await?;
    let rows = sqlx::query("SELECT a.user_id,a.node_id,a.uuid,a.credential,n.protocol_config FROM accesses a JOIN nodes n ON n.id=a.node_id ORDER BY a.node_id,a.user_id")
        .fetch_all(&pool).await?;
    assert_eq!(rows.len(), 12);
    let mut secrets = std::collections::BTreeSet::new();
    for row in &rows {
        let config: apsp_compiler::ProtocolConfig =
            serde_json::from_value(row.get("protocol_config"))?;
        let secret: String = row.get("credential");
        assert_eq!(STANDARD.decode(&secret)?.len(), config.credential_size());
        assert!(secrets.insert(secret));
    }
    let first_credentials: Vec<(i64, Uuid, String)> = sqlx::query_as(
        "SELECT node_id,uuid,credential FROM accesses WHERE user_id=$1 ORDER BY node_id",
    )
    .bind(uid)
    .fetch_all(&pool)
    .await?;
    // Overlapping direct grants reuse rather than replace group-generated credentials.
    panel.grant(&cookie, uid, nodes[1]).await?;
    set_policies(&panel, &cookie, uid, &[]).await?;
    let direct_secret: String =
        sqlx::query_scalar("SELECT credential FROM accesses WHERE user_id=$1 AND node_id=$2")
            .bind(uid)
            .bind(nodes[1])
            .fetch_one(&pool)
            .await?;
    assert_eq!(direct_secret, first_credentials[1].2);
    set_policies(&panel, &cookie, uid, &[group]).await?;
    let current_credentials: Vec<(i64, Uuid, String)> = sqlx::query_as(
        "SELECT node_id,uuid,credential FROM accesses WHERE user_id=$1 ORDER BY node_id",
    )
    .bind(uid)
    .fetch_all(&pool)
    .await?;
    assert_ne!(current_credentials[0].2, first_credentials[0].2);
    assert_eq!(current_credentials[1], first_credentials[1]);
    let views = call(
        &panel,
        &cookie,
        Method::GET,
        &format!("/users/{uid}/accesses"),
        None,
    )
    .await?;
    assert!(
        views
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v.get("credential").is_none())
    );
    let plan = create_plan(&panel, &cookie, Some("6")).await?;
    assign(&panel, &cookie, uid, plan, Uuid::new_v4()).await?;
    entitlements::refresh(&pool, now_timestamp()).await?;
    panel.publish_now().await?;
    applied(&pool).await?;
    let native = latest_config(&pool, server).await?;
    assert_eq!(native["inbounds"].as_array().unwrap().len(), 6);
    assert_eq!(native["certificate_providers"].as_array().unwrap().len(), 1);
    assert_eq!(
        native["experimental"]["v2ray_api"]["stats"]["users"]
            .as_array()
            .unwrap()
            .len(),
        12
    );
    let sub = format!(
        "{}?format=singbox",
        first["subscription_url"].as_str().unwrap()
    );
    let client: Value = panel
        .client
        .get(&sub)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        client["outbounds"][0]["outbounds"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    assert!(client.get("certificate_providers").is_none());
    // Mixed protocols still require JSON instead of silently truncated link lists.
    assert_eq!(
        panel
            .client
            .get(first["subscription_url"].as_str().unwrap())
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    for node in &nodes {
        apsp_panel::usage::ingest(
            &panel.state,
            server,
            usage(uid, *node, now_timestamp(), 1, 0),
        )
        .await?;
    }
    assert_eq!(
        entitlement(&panel, &cookie, uid).await?["status"],
        "exhausted"
    );
    assert!(eligible(&pool, uid, now_timestamp()).await?.is_empty());
    assert_eq!(eligible(&pool, other, now_timestamp()).await?, nodes);
    assert_eq!(
        panel.client.get(&sub).send().await?.status(),
        StatusCode::CONFLICT
    );
    entitlements::refresh(&pool, now_timestamp()).await?;
    panel.publish_now().await?;
    let native = latest_config(&pool, server).await?;
    assert_eq!(
        native["experimental"]["v2ray_api"]["stats"]["users"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    assert!(
        native["inbounds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["users"].as_array().unwrap().len() == 1)
    );
    assert_eq!(native["certificate_providers"].as_array().unwrap().len(), 1);
    let stored: Vec<(i64, Uuid, String)> = sqlx::query_as(
        "SELECT node_id,uuid,credential FROM accesses WHERE user_id=$1 ORDER BY node_id",
    )
    .bind(uid)
    .fetch_all(&pool)
    .await?;
    assert_eq!(stored, current_credentials);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn reality_chains_reject_non_reality_endpoints_but_can_share_hosts_with_modern_nodes(
    pool: PgPool,
) -> Result<()> {
    let panel =
        TestPanel::start_with_public_url(pool.clone(), Some("https://panel.example")).await?;
    let cookie = panel.admin_cookie().await?;
    let a = panel.create_server(&cookie, "Entry server").await?;
    let b = panel.create_server(&cookie, "Exit server").await?;
    let entry = id(&panel.create_node(&cookie, a, "Entry").await?)?;
    let exit = id(&panel.create_node(&cookie, b, "Exit").await?)?;
    let modern_entry = modern_node(&panel, &cookie, a, "anytls").await?;
    let modern_exit = modern_node(&panel, &cookie, b, "shadowsocks2022").await?;
    super::chains::ordered_negative_inputs(&panel).await?;
    for (n, e) in [
        (entry, modern_exit),
        (modern_entry, exit),
        (modern_entry, modern_exit),
    ] {
        let response = panel
            .admin(
                Method::POST,
                &format!("{ROOT}/chains"),
                &cookie,
                Some(json!({"name":"Not a Reality pair","entry_node_id":n,"exit_node_id":e})),
            )
            .await?;
        // Two-hop creation is closed (ADR 0079 phase 3, S1d).
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }
    let chain = id(&panel
        .import_legacy_chain(&cookie, "Reality pair", entry, exit)
        .await?)?;
    let user = panel.create_user(&cookie, "Mixed route user").await?;
    let uid = id(&user)?;
    let group = create_policy(&panel, &cookie, &[modern_entry, modern_exit], &[chain]).await?;
    set_policies(&panel, &cookie, uid, &[group]).await?;
    entitlements::refresh(&pool, now_timestamp()).await?;
    panel.publish_now().await?;
    let native = latest_config(&pool, a).await?;
    assert_eq!(
        native["route"]["rules"][0]["inbound"],
        json!([format!("node-{entry}")])
    );
    assert!(
        native["inbounds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["type"] == "anytls")
    );
    assert_eq!(native["certificate_providers"].as_array().unwrap().len(), 1);
    let native_exit = latest_config(&pool, b).await?;
    assert!(
        native_exit["inbounds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["type"] == "shadowsocks")
    );
    assert_eq!(
        native_exit["experimental"]["v2ray_api"]["stats"]["users"],
        json!([format!("u{uid}_n{modern_exit}")])
    );
    applied(&pool).await?;
    let token = user["subscription_token"]
        .as_str()
        .context("existing subscription token")?;
    let published = reqwest::Url::parse(
        user["subscription_url"]
            .as_str()
            .context("returned subscription URL")?,
    )?;
    ensure!(
        published.origin().ascii_serialization() == "https://panel.example"
            && published.path() == format!("/sub/{token}")
            && published.query().is_none()
            && published.fragment().is_none(),
        "returned subscription URL must preserve the configured origin and token path"
    );
    // The HTTPS origin is reserved TEST_ONLY configuration for signed probes;
    // request its exact public subscription path from this fixture's loopback listener.
    let sub = format!("{}{}?format=singbox", panel.base, published.path());
    let client: Value = panel
        .client
        .get(&sub)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        client["outbounds"][0]["outbounds"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let relay: Uuid = sqlx::query_scalar("SELECT relay_uuid FROM singbox_chains WHERE id=$1")
        .bind(chain)
        .fetch_one(&pool)
        .await?;
    assert!(!client.to_string().contains(&relay.to_string()));
    Ok(())
}

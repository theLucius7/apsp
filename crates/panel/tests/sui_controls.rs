#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
use anyhow::Result;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
const ROOT: &str = "/api/plugins/sing-box";

#[sqlx::test]
async fn pause_is_versioned_marks_hosts_and_preserves_identity(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = panel.create_user(&cookie, "暂停测试").await?;
    let id = user["id"].as_i64().unwrap();
    let server = panel.create_server(&cookie, "暂停主机").await?;
    let node = panel.create_node(&cookie, server, "暂停节点").await?;
    panel
        .grant(&cookie, id, node["id"].as_i64().unwrap())
        .await?;
    let before: Value = sqlx::query_scalar("SELECT to_jsonb(u) FROM users u WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await?;
    sqlx::query("UPDATE servers SET dirty_at=NULL WHERE id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    let path = format!("{ROOT}/users/{id}/control");
    assert_eq!(
        panel
            .client
            .get(format!("{}{path}", panel.base))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let paused = panel
        .admin(
            Method::PUT,
            &path,
            &cookie,
            Some(json!({"expected_revision":0,"paused":true})),
        )
        .await?;
    assert!(paused.status().is_success(), "{}", paused.text().await?);
    let entitlement: (String, bool) =
        sqlx::query_as("SELECT status,allowed FROM singbox_entitlements($1) WHERE user_id=$2")
            .bind(apsp_protocol::now_timestamp())
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(entitlement, ("paused".into(), false));
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT dirty_at IS NOT NULL FROM servers WHERE id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?
    );
    assert_eq!(
        panel
            .admin(
                Method::PUT,
                &path,
                &cookie,
                Some(json!({"expected_revision":0,"paused":false}))
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    panel
        .admin(
            Method::PUT,
            &path,
            &cookie,
            Some(json!({"expected_revision":1,"paused":false})),
        )
        .await?
        .error_for_status()?;
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT allowed FROM singbox_entitlements($1) WHERE user_id=$2"
        )
        .bind(apsp_protocol::now_timestamp())
        .bind(id)
        .fetch_one(&pool)
        .await?
    );
    let after: Value = sqlx::query_scalar("SELECT to_jsonb(u) FROM users u WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(before, after);
    Ok(())
}

#[sqlx::test]
async fn profiles_redact_secrets_preserve_them_on_edit_and_reject_stale_writes(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "配置主机").await?;
    panel.enable_plugin(&cookie, server).await?;
    let path = format!("{ROOT}/servers/{server}/profile");
    let definition = json!({"outbounds":[{"type":"socks","tag":"extra-exit","server":"proxy.example.com","server_port":1080,"username":"fixture","password":"TEST_ONLY-private-password"}]});
    let response = panel
        .admin(
            Method::PUT,
            &path,
            &cookie,
            Some(json!({"expected_revision":0,"definition":definition})),
        )
        .await?;
    assert!(response.status().is_success(), "{}", response.text().await?);
    let mut read: Value = panel
        .admin(Method::GET, &path, &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(!read.to_string().contains("TEST_ONLY-private"));
    read["definition"]["outbounds"][0]["server_port"] = json!(1081);
    let response = panel
        .admin(
            Method::PUT,
            &path,
            &cookie,
            Some(json!({"expected_revision":1,"definition":read["definition"]})),
        )
        .await?;
    assert!(response.status().is_success(), "{}", response.text().await?);
    let stored: Value =
        sqlx::query_scalar("SELECT definition FROM singbox_server_profiles WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        stored["outbounds"][0]["password"],
        "TEST_ONLY-private-password"
    );
    assert_eq!(stored["outbounds"][0]["server_port"], 1081);
    assert_eq!(
        panel
            .admin(
                Method::PUT,
                &path,
                &cookie,
                Some(json!({"expected_revision":1,"definition":{}}))
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        panel
            .admin(
                Method::PUT,
                &path,
                &cookie,
                Some(json!({"expected_revision":2,"definition":{"experimental":{}}}))
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let events: Vec<Value> = sqlx::query_scalar(
        "SELECT detail FROM singbox_operation_events WHERE action='server_profile_updated'",
    )
    .fetch_all(&pool)
    .await?;
    assert!(!format!("{events:?}").contains("private-password"));
    Ok(())
}

#[sqlx::test]
async fn sessions_derive_only_the_selected_users_applied_principals(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let id = panel.create_user(&cookie, "连接测试").await?["id"]
        .as_i64()
        .unwrap();
    let server = panel.create_server(&cookie, "连接主机").await?;
    let path = format!("{ROOT}/users/{id}/sessions/{server}");
    let requested = json!({"expected_revision":3,"close":false,"ids":[]});
    assert_eq!(
        panel
            .admin(Method::POST, &path, &cookie, Some(requested.clone()))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let principal = format!("u{id}_n1");
    let config = json!({"experimental":{"v2ray_api":{"stats":{"users":[principal,format!("u{id}_n2"),format!("u{}0_n1",id),"u999999_n1"]}}}});
    let bundle = json!({"files":{"config.json":config.to_string()}}).to_string();
    sqlx::query("INSERT INTO deployments(server_id,module,rev,bundle,bundle_sha256,created_at) VALUES($1,'singbox',3,$2,$3,0)").bind(server).bind(bundle).bind("a".repeat(64)).execute(&pool).await?;
    sqlx::query("INSERT INTO server_module_status(server_id,module,applied_rev,target_rev,healthy) VALUES($1,'singbox',3,3,true)").bind(server).execute(&pool).await?;
    sqlx::query("UPDATE servers SET dirty_at=NULL,last_seen=$2,capabilities=$3 WHERE id=$1")
        .bind(server)
        .bind(apsp_protocol::now_timestamp())
        .bind(json!([apsp_protocol::RUNTIME_SESSIONS_CAPABILITY]))
        .execute(&pool)
        .await?;
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &path,
                &cookie,
                Some(json!({"expected_revision":2,"close":false}))
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let response = panel
        .admin(Method::POST, &path, &cookie, Some(requested.clone()))
        .await?;
    assert!(response.status().is_success(), "{}", response.text().await?);
    let spec: Value = sqlx::query_scalar("SELECT spec FROM runtime_operations WHERE server_id=$1")
        .bind(server)
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        spec["session_filter"]["principals"],
        json!([principal, format!("u{id}_n2")])
    );
    assert_eq!(spec["expected_revision"], 3);
    assert_eq!(
        panel
            .admin(Method::POST, &path, &cookie, Some(requested))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    Ok(())
}

#[sqlx::test]
async fn dns_certificate_secrets_are_write_only_and_reuse_stays_on_one_host(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "证书主机").await?;
    let other = panel.create_server(&cookie, "其他主机").await?;
    for host in [server, other] {
        panel.enable_plugin(&cookie, host).await?;
    }
    let secret = "TEST_ONLY-cloudflare-dns-credential";
    let definition = json!({"name":"DNS 证书","server_id":server,"public_host":"proxy.example.com","sni":"proxy.example.com","protocol_config":{"type":"vless-tls","tls":{"mode":"dns01","email":"admin@example.com","domains":["*.example.com"],"api_token":secret}}});
    let response = panel
        .admin(
            Method::POST,
            &format!("{ROOT}/nodes"),
            &cookie,
            Some(definition),
        )
        .await?;
    let status = response.status();
    let text = response.text().await?;
    assert!(status.is_success(), "{status}: {text}");
    assert!(!text.contains(secret));
    let node: Value = serde_json::from_str(&text)?;
    let id = node["id"].as_i64().unwrap();
    let mut reuse = json!({"name":"复用证书","server_id":server,"public_host":"reuse.example.com","sni":"reuse.example.com","protocol_config":{"type":"trojan","tls":{"mode":"reuse","node_id":id}}});
    let response = panel
        .admin(
            Method::POST,
            &format!("{ROOT}/nodes"),
            &cookie,
            Some(reuse.clone()),
        )
        .await?;
    let status = response.status();
    let text = response.text().await?;
    assert!(status.is_success(), "{status}: {text}");
    assert!(!text.contains(secret));
    let reused: Value = serde_json::from_str(&text)?;
    let stored: Value = sqlx::query_scalar("SELECT protocol_config FROM nodes WHERE id=$1")
        .bind(reused["id"].as_i64().unwrap())
        .fetch_one(&pool)
        .await?;
    assert_eq!(stored["tls"]["api_token"], secret);
    reuse["server_id"] = json!(other);
    assert_eq!(
        panel
            .admin(Method::POST, &format!("{ROOT}/nodes"), &cookie, Some(reuse))
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    Ok(())
}

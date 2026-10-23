#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

const ROOT: &str = "/api/plugins/sing-box";

async fn preview(panel: &TestPanel, cookie: &str, user: i64, format: &str) -> Result<Value> {
    let response = panel
        .admin(
            Method::GET,
            &format!("{ROOT}/users/{user}/subscription?format={format}"),
            cookie,
            None,
        )
        .await?
        .error_for_status()?;
    assert_eq!(response.headers()["cache-control"], "no-store");
    Ok(response.json().await?)
}

#[sqlx::test]
async fn previews_and_downloads_use_the_same_applied_seven_protocol_snapshot(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Subscriptions").await?;
    panel.enable_plugin(&cookie, server).await?;
    let user = panel.create_user(&cookie, "First").await?;
    let uid = id(&user)?;
    let other = panel.create_user(&cookie, "Second").await?;
    let mut secrets = Vec::new();
    for kind in [
        "vless-reality",
        "hysteria2",
        "shadowsocks2022",
        "tuic",
        "anytls",
        "naive",
        "snell-v6",
    ] {
        let mut protocol = json!({"type":kind});
        if ["hysteria2", "tuic", "anytls", "naive"].contains(&kind) {
            protocol["tls"] =
                json!({"mode":"acme","email":"admin@example.com","challenge":"http-01"});
        }
        if kind == "shadowsocks2022" {
            protocol["method"] = json!("2022-blake3-aes-128-gcm");
        }
        let node: Value = panel.admin(Method::POST,&format!("{ROOT}/nodes"),&cookie,Some(json!({"server_id":server,"name":kind,"sni":if ["shadowsocks2022","snell-v6"].contains(&kind) { "" } else { "proxy.example.com" },"public_host":"proxy.example.com","protocol_config":protocol}))).await?.error_for_status()?.json().await?;
        panel.grant(&cookie, uid, id(&node)?).await?;
        panel.grant(&cookie, id(&other)?, id(&node)?).await?;
    }
    let response = panel
        .client
        .get(format!("{}{ROOT}/users/{uid}/subscription", panel.base))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let empty = preview(&panel, &cookie, uid, "singbox").await?;
    assert_eq!(empty["status"], "empty");
    assert_eq!(empty["eligible_nodes"], 7);
    assert!(empty["content"].is_null());
    panel.publish_now().await?;
    assert_eq!(
        preview(&panel, &cookie, uid, "singbox").await?["status"],
        "empty"
    );
    sqlx::query("UPDATE server_module_status SET applied_rev=target_rev,healthy=TRUE")
        .execute(&pool)
        .await?;
    let credentials: Vec<(String, String)> =
        sqlx::query_as("SELECT uuid::text,credential FROM accesses WHERE user_id=$1")
            .bind(id(&other)?)
            .fetch_all(&pool)
            .await?;
    for (uuid, credential) in credentials {
        secrets.push(uuid);
        if !credential.is_empty() {
            secrets.push(credential);
        }
    }
    let ready = preview(&panel, &cookie, uid, "singbox").await?;
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["granted_nodes"], 7);
    assert_eq!(ready["available_formats"], json!(["singbox"]));
    assert_eq!(ready["ready_nodes"].as_array().unwrap().len(), 7);
    assert_eq!(ready["subscription_url"], user["subscription_url"]);
    let content = ready["content"].as_str().unwrap();
    for secret in secrets {
        assert!(!content.contains(&secret));
    }
    assert!(!content.contains("private_key"));
    assert!(!content.contains("admin@example.com"));
    let config: Value = serde_json::from_str(content)?;
    assert_eq!(config["outbounds"].as_array().unwrap().len(), 9);
    let unavailable = preview(&panel, &cookie, uid, "links").await?;
    assert_eq!(unavailable["status"], "format_unavailable");
    assert!(unavailable["content"].is_null());
    let download = panel
        .client
        .get(format!(
            "{}?format=singbox&download=true",
            user["subscription_url"].as_str().unwrap()
        ))
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(
        download.headers()["content-disposition"],
        format!("attachment; filename=\"apsp-{uid}.json\"")
    );
    assert_eq!(download.headers()["referrer-policy"], "no-referrer");
    assert_eq!(download.text().await?, content);
    let updated: Value = panel
        .admin(
            Method::POST,
            &format!("{ROOT}/users/{uid}/subscription/reset"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        preview(&panel, &cookie, uid, "singbox").await?["subscription_url"],
        updated["subscription_url"]
    );
    assert_eq!(
        panel
            .client
            .get(format!(
                "{}?format=singbox",
                user["subscription_url"].as_str().unwrap()
            ))
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[sqlx::test]
async fn paused_and_expired_entitlements_cannot_return_preview_credentials(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Expiry").await?;
    let node = id(&panel.create_node(&cookie, server, "Reality").await?)?;
    let uid = id(&panel.create_user(&cookie, "Member").await?)?;
    panel.grant(&cookie, uid, node).await?;
    panel.publish_now().await?;
    sqlx::query("UPDATE server_module_status SET applied_rev=target_rev,healthy=TRUE")
        .execute(&pool)
        .await?;
    let ready = preview(&panel, &cookie, uid, "singbox").await?;
    assert_eq!(ready["available_formats"], json!(["singbox", "links"]));
    panel
        .admin(
            Method::PATCH,
            &format!("{ROOT}/nodes/{node}"),
            &cookie,
            Some(json!({"enabled":false})),
        )
        .await?
        .error_for_status()?;
    let paused = preview(&panel, &cookie, uid, "singbox").await?;
    assert_eq!(paused["status"], "empty");
    assert_eq!(paused["granted_nodes"], 1);
    assert!(paused["content"].is_null());
    panel
        .admin(
            Method::PATCH,
            &format!("{ROOT}/nodes/{node}"),
            &cookie,
            Some(json!({"enabled":true})),
        )
        .await?
        .error_for_status()?;
    let package: Value = panel.admin(Method::POST,&format!("{ROOT}/package-groups"),&cookie,Some(json!({"name":"Plan","monthly_bytes":"18446744073709551615","reset_day":1,"reset_hour":0,"reset_minute":0,"timezone":"UTC","duration_days":1}))).await?.error_for_status()?.json().await?;
    panel
        .admin(
            Method::POST,
            &format!("{ROOT}/users/{uid}/package"),
            &cookie,
            Some(json!({"package_group_id":id(&package)?,"request_id":uuid::Uuid::new_v4()})),
        )
        .await?
        .error_for_status()?;
    sqlx::query("UPDATE singbox_package_assignments SET starts_at=$2-172800,expires_at=$2-86400 WHERE user_id=$1").bind(uid).bind(apsp_protocol::now_timestamp()).execute(&pool).await?;
    let expired = preview(&panel, &cookie, uid, "singbox").await?;
    assert_eq!(expired["status"], "blocked");
    assert_eq!(expired["entitlement"]["status"], "expired");
    assert_eq!(
        expired["entitlement"]["monthly_bytes"],
        "18446744073709551615"
    );
    assert!(expired["content"].is_null());
    assert_eq!(expired["ready_nodes"], json!([]));
    assert!(expired["message"].as_str().unwrap().contains("到期"));
    Ok(())
}

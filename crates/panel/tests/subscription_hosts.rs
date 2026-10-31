#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_panel::plugins::singbox::subscription_hosts::{DnsApi, reconcile};
use axum::{
    Json, Router,
    extract::Path,
    routing::{delete, post},
};
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode, header};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::{Arc, Mutex};

const ROOT: &str = "/api/plugins/sing-box";
const ZONE: &str = "example.test";

/// A stand-in for Cloudflare's DNS API that remembers what was asked of it.
async fn fake_cloudflare() -> Result<(String, Arc<Mutex<Vec<String>>>)> {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let created = calls.clone();
    let deleted = calls.clone();
    let app = Router::new()
        .route(
            "/zones/{zone}/dns_records",
            post(move |Path(_): Path<String>, Json(body): Json<Value>| {
                let created = created.clone();
                async move {
                    let mut calls = created.lock().unwrap();
                    calls.push(format!("create {}", body["name"].as_str().unwrap()));
                    assert_eq!(body["proxied"], true);
                    Json(json!({"success":true,"result":{"id":format!("record-{}", calls.len())}}))
                }
            }),
        )
        .route(
            "/zones/{zone}/dns_records/{record}",
            delete(move |Path((_, record)): Path<(String, String)>| {
                let deleted = deleted.clone();
                async move {
                    deleted.lock().unwrap().push(format!("delete {record}"));
                    Json(json!({"success":true,"result":{"id":record}}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Ok((format!("http://{address}"), calls))
}

async fn enable(pool: &PgPool, legacy_paths: bool) -> Result<()> {
    let credential = apsp_panel_host::control_center::credentials::store_generated(
        pool,
        "fixture",
        "dns",
        json!({"api_token":"TEST_ONLY_TOKEN"}),
    )
    .await?;
    sqlx::query("INSERT INTO singbox_subscription_domain(enabled,credential_id,zone_id,zone_name,host_prefix,target,legacy_paths,updated_at) VALUES(TRUE,$1,'zone',$2,'apsp-','panel.example.test',$3,0)")
        .bind(credential)
        .bind(ZONE)
        .bind(legacy_paths)
        .execute(pool)
        .await?;
    Ok(())
}

async fn host_of(pool: &PgPool, user: i64) -> Result<(String, String)> {
    let (slug, path): (String, String) =
        sqlx::query_as("SELECT slug,path_token FROM singbox_subscription_hosts WHERE user_id=$1")
            .bind(user)
            .fetch_one(pool)
            .await?;
    Ok((format!("apsp-{slug}.{ZONE}"), path))
}

async fn fetch(panel: &TestPanel, host: &str, path: &str) -> Result<reqwest::Response> {
    Ok(panel
        .client
        .get(format!("{}{path}", panel.base))
        .header(header::HOST, host)
        .send()
        .await?)
}

#[sqlx::test]
async fn a_users_host_answers_only_for_that_user(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let first = id(&panel.create_user(&cookie, "First").await?)?;
    let second = id(&panel.create_user(&cookie, "Second").await?)?;
    enable(&pool, true).await?;
    let (dns, calls) = fake_cloudflare().await?;
    reconcile(&pool, &DnsApi::new(&dns)).await?;
    assert_eq!(
        calls.lock().unwrap().len(),
        2,
        "every user gets one proxied record"
    );
    let (host, path) = host_of(&pool, first).await?;
    let (_, other_path) = host_of(&pool, second).await?;

    assert_eq!(
        fetch(&panel, &host, &format!("/{path}")).await?.status(),
        StatusCode::OK
    );
    assert_eq!(
        fetch(&panel, &host, &format!("/{path}/login"))
            .await?
            .status(),
        StatusCode::OK
    );
    for wrong in [
        "/",
        "/api/dashboard/access",
        "/api/control-center/me",
        "/api/servers",
        "/sub/x",
        "/nope",
        &format!("/{other_path}"),
        &format!("/{other_path}/login"),
    ] {
        let response = fetch(&panel, &host, wrong).await?;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{wrong} must not exist on a user's host"
        );
        assert!(
            response.text().await?.is_empty(),
            "{wrong} must look like nothing"
        );
    }
    // An address that was never issued looks the same as one that was.
    assert_eq!(
        fetch(
            &panel,
            &format!("apsp-aaaaaaaaaaaaaaaa.{ZONE}"),
            &format!("/{path}")
        )
        .await?
        .status(),
        StatusCode::NOT_FOUND
    );
    // Other hostnames keep reaching the panel.
    assert_eq!(
        panel
            .client
            .get(format!("{}/api/dashboard/access", panel.base))
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    Ok(())
}

#[sqlx::test]
async fn reset_releases_the_old_address_and_its_record(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = id(&panel.create_user(&cookie, "User").await?)?;
    enable(&pool, true).await?;
    let (dns, calls) = fake_cloudflare().await?;
    reconcile(&pool, &DnsApi::new(&dns)).await?;
    let (host, path) = host_of(&pool, user).await?;
    let listed: Vec<Value> = panel
        .admin(Method::GET, &format!("{ROOT}/users"), &cookie, None)
        .await?
        .json()
        .await?;
    assert_eq!(
        listed[0]["subscription_url"],
        format!("https://{host}/{path}")
    );

    let reset: Value = panel
        .admin(
            Method::POST,
            &format!("{ROOT}/users/{user}/subscription/reset"),
            &cookie,
            None,
        )
        .await?
        .json()
        .await?;
    let (new_host, new_path) = host_of(&pool, user).await?;
    assert_ne!(host, new_host);
    assert_ne!(path, new_path);
    assert_eq!(
        fetch(&panel, &host, &format!("/{path}")).await?.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fetch(&panel, &host, &format!("/{path}/login"))
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fetch(&panel, &new_host, &format!("/{new_path}"))
            .await?
            .status(),
        StatusCode::OK
    );
    reconcile(&pool, &DnsApi::new(&dns)).await?;
    let log = calls.lock().unwrap().clone();
    assert!(
        log.iter().any(|call| call == "delete record-1"),
        "the old record is released: {log:?}"
    );
    assert!(
        log.iter().any(|call| call == &format!("create {new_host}")),
        "the new host has a record: {log:?}"
    );
    assert!(reset["subscription_url"].is_string());
    Ok(())
}

#[sqlx::test]
async fn closing_old_paths_leaves_only_the_users_host(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = id(&panel.create_user(&cookie, "User").await?)?;
    let token: String = sqlx::query_scalar("SELECT subscription_token FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(&pool)
        .await?;
    let old = format!("{}/sub/{token}", panel.base);
    assert_eq!(
        panel.client.get(&old).send().await?.status(),
        StatusCode::OK
    );
    enable(&pool, true).await?;
    let (dns, _) = fake_cloudflare().await?;
    reconcile(&pool, &DnsApi::new(&dns)).await?;
    assert_eq!(
        panel.client.get(&old).send().await?.status(),
        StatusCode::OK,
        "old clients keep working until the administrator closes the old path"
    );
    sqlx::query("UPDATE singbox_subscription_domain SET legacy_paths=FALSE")
        .execute(&pool)
        .await?;
    assert_eq!(
        panel.client.get(&old).send().await?.status(),
        StatusCode::NOT_FOUND
    );
    let (host, path) = host_of(&pool, user).await?;
    assert_eq!(
        fetch(&panel, &host, &format!("/{path}")).await?.status(),
        StatusCode::OK
    );
    Ok(())
}

#[sqlx::test]
async fn the_user_overview_never_carries_credentials(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = id(&panel.create_user(&cookie, "User").await?)?;
    let token: String = sqlx::query_scalar("SELECT subscription_token FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(&pool)
        .await?;
    let text = panel
        .admin(
            Method::GET,
            &format!("{ROOT}/users/overview"),
            &cookie,
            None,
        )
        .await?
        .text()
        .await?;
    assert!(!text.contains(&token));
    let rows: Vec<Value> = serde_json::from_str(&text)?;
    assert_eq!(rows[0]["id"], user);
    assert_eq!(rows[0]["status"], "unmetered");
    assert_eq!(rows[0]["group_ids"], json!([]));
    assert_eq!(rows[0]["portal_keys"], 0);
    Ok(())
}

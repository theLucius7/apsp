#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_panel::auth::{hash_token, random_token};
use apsp_panel::plugins::singbox::subscription_hosts::{DnsApi, reconcile};
use axum::{
    Json, Router,
    extract::{Path, RawQuery},
    routing::{delete, post},
};
use business_support::{TestPanel, id};
use reqwest::{Method, StatusCode, header};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

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

/// A stand-in for the retained renderer that still serves migrated users.
async fn fake_renderer() -> Result<(String, Arc<Mutex<Vec<String>>>)> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let app = Router::new().route(
        "/assets/{token}",
        axum::routing::get(
            move |Path(token): Path<String>,
                  RawQuery(query): RawQuery,
                  headers: axum::http::HeaderMap| {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push(format!(
                        "{token} {} {}",
                        query.unwrap_or_default(),
                        headers
                            .get("user-agent")
                            .and_then(|agent| agent.to_str().ok())
                            .unwrap_or_default()
                    ));
                    (
                        [
                            ("subscription-userinfo", "upload=1; download=2; total=3"),
                            ("content-type", "text/yaml"),
                        ],
                        "TEST_ONLY retained nodes",
                    )
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Ok((format!("http://{address}/assets/"), seen))
}

async fn bridge(pool: &PgPool, user: i64, base: &str) -> Result<()> {
    let import = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO singbox_nexus_imports VALUES($1,'fixture',repeat('a',64),'{}',1,1)")
        .bind(import)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO singbox_nexus_identities(user_id,import_id,legacy_uuid,original_state,used_upload,used_download) VALUES($1,$2,$3,'active',0,0)")
        .bind(user)
        .bind(import)
        .bind(uuid::Uuid::new_v4())
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO singbox_nexus_bridge(singleton,import_id,subscription_base,enabled,last_sync_at,activated_at) VALUES(TRUE,$1,$2,TRUE,1,1)")
        .bind(import)
        .bind(base)
        .execute(pool)
        .await?;
    Ok(())
}

#[sqlx::test]
async fn a_migrated_user_gets_the_working_link_and_their_own_address(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = id(&panel.create_user(&cookie, "Migrated").await?)?;
    let token: String = sqlx::query_scalar("SELECT subscription_token FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(&pool)
        .await?;
    let (renderer, seen) = fake_renderer().await?;
    bridge(&pool, user, &renderer).await?;
    // The panel's own path has no nodes for them, so it must not be what people are given.
    let listed: Vec<Value> = panel
        .admin(Method::GET, &format!("{ROOT}/users"), &cookie, None)
        .await?
        .json()
        .await?;
    assert_eq!(listed[0]["subscription_url"], format!("{renderer}{token}"));

    enable(&pool, true).await?;
    let (dns, _) = fake_cloudflare().await?;
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

    let answer = panel
        .client
        .get(format!("{}/{path}?flag=meta", panel.base))
        .header(header::HOST, &host)
        .header(header::USER_AGENT, "clash-verge/2.0")
        .send()
        .await?;
    assert_eq!(answer.status(), StatusCode::OK);
    assert_eq!(
        answer.headers()["subscription-userinfo"],
        "upload=1; download=2; total=3"
    );
    assert_eq!(answer.text().await?, "TEST_ONLY retained nodes");
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        [format!("{token} flag=meta clash-verge/2.0")],
        "the renderer still decides the format"
    );

    // Reset releases their own address; the token stays with the renderer.
    panel
        .admin(
            Method::POST,
            &format!("{ROOT}/users/{user}/subscription/reset"),
            &cookie,
            None,
        )
        .await?;
    let (new_host, new_path) = host_of(&pool, user).await?;
    assert_ne!(host, new_host);
    assert_eq!(
        fetch(&panel, &host, &format!("/{path}")).await?.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fetch(&panel, &new_host, &format!("/{new_path}"))
            .await?
            .status(),
        StatusCode::OK
    );
    let after: String = sqlx::query_scalar("SELECT subscription_token FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(&pool)
        .await?;
    assert_eq!(after, token);
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

/// The subscription fetches recorded for one user, oldest first.
async fn fetches(pool: &PgPool, user: i64) -> Result<Vec<Value>> {
    Ok(sqlx::query_scalar(
        "SELECT detail FROM singbox_operation_events WHERE action='subscription_fetch' AND user_id=$1 ORDER BY id",
    )
    .bind(user)
    .fetch_all(pool)
    .await?)
}

#[sqlx::test]
async fn every_subscription_fetch_is_recorded_including_relayed_ones(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let native = id(&panel.create_user(&cookie, "Native").await?)?;
    let migrated = id(&panel.create_user(&cookie, "Migrated").await?)?;
    let (renderer, _) = fake_renderer().await?;
    bridge(&pool, migrated, &renderer).await?;
    enable(&pool, true).await?;
    let (dns, _) = fake_cloudflare().await?;
    reconcile(&pool, &DnsApi::new(&dns)).await?;

    // The panel answers the native user's own host; a Clash profile needs at least
    // one node, so that request cannot be served and is recorded as unavailable.
    let (host, path) = host_of(&pool, native).await?;
    assert_eq!(
        fetch(&panel, &host, &format!("/{path}")).await?.status(),
        StatusCode::OK
    );
    let answer = panel
        .client
        .get(format!("{}/{path}", panel.base))
        .header(header::HOST, &host)
        .header(header::USER_AGENT, "mihomo/1.19")
        .send()
        .await?;
    assert_eq!(answer.status(), StatusCode::CONFLICT);
    let recorded = fetches(&pool, native).await?;
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0]["format"], "links");
    assert_eq!(recorded[0]["result"], "generated");
    assert_eq!(recorded[1]["format"], "clash");
    assert_eq!(recorded[1]["result"], "unavailable");

    // The migrated user's host is relayed to the retained renderer, and that fetch is
    // kept like any other, without node counts.
    let (migrated_host, migrated_path) = host_of(&pool, migrated).await?;
    assert_eq!(
        fetch(&panel, &migrated_host, &format!("/{migrated_path}"))
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        fetches(&pool, migrated).await?,
        [
            json!({"format":"renderer","managed_nodes":null,"external_nodes":null,"result":"relayed"})
        ]
    );

    // A renderer that does not answer is recorded as unavailable; the client gets a gateway error.
    sqlx::query("UPDATE singbox_nexus_bridge SET subscription_base='http://127.0.0.1:9/assets/'")
        .execute(&pool)
        .await?;
    assert_eq!(
        fetch(&panel, &migrated_host, &format!("/{migrated_path}"))
            .await?
            .status(),
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(fetches(&pool, migrated).await?[1]["result"], "unavailable");

    // With fetch records switched off, nothing more is kept.
    sqlx::query("UPDATE singbox_operation_privacy SET subscription_days=0")
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE singbox_nexus_bridge SET subscription_base=$1")
        .bind(&renderer)
        .execute(&pool)
        .await?;
    assert_eq!(
        fetch(&panel, &migrated_host, &format!("/{migrated_path}"))
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(fetches(&pool, migrated).await?.len(), 2);
    Ok(())
}

/// A signed-in portal session for an existing proxy user, as a Passkey sign-in leaves it.
async fn portal_session(pool: &PgPool, user: i64) -> Result<(Uuid, String)> {
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
    let now = apsp_protocol::now_timestamp();
    sqlx::query("INSERT INTO singbox_portal_sessions(token_hash,account_id,expires_at,verified_at) VALUES($1,$2,$3,$4)")
        .bind(hash_token(&token))
        .bind(account)
        .bind(now + 3600)
        .bind(now)
        .execute(pool)
        .await?;
    Ok((account, format!("apsp_proxy_session={token}")))
}

#[sqlx::test]
async fn a_users_own_address_takes_their_account_changes(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = id(&panel.create_user(&cookie, "Own").await?)?;
    enable(&pool, true).await?;
    let (dns, _) = fake_cloudflare().await?;
    reconcile(&pool, &DnsApi::new(&dns)).await?;
    let (host, _) = host_of(&pool, user).await?;
    let (account, session) = portal_session(&pool, user).await?;
    let save = |origin: String| {
        panel
            .client
            .put(format!(
                "{}{ROOT}/portal/{account}/account/reminders",
                panel.base
            ))
            .header(header::HOST, &host)
            .header(header::ORIGIN, origin)
            .header(header::COOKIE, &session)
            .json(&json!({"remind_expire": true, "remind_traffic": false}))
            .send()
    };
    // The portal page was opened at the user's own address, so that is its origin.
    let accepted = save(format!("https://{host}")).await?;
    let status = accepted.status();
    assert_eq!(status, StatusCode::OK, "{}", accepted.text().await?);
    let saved: (bool, bool) =
        sqlx::query_as("SELECT remind_expire,remind_traffic FROM singbox_members WHERE user_id=$1")
            .bind(user)
            .fetch_one(&pool)
            .await?;
    assert_eq!(saved, (true, false));
    // A page from anywhere else cannot write through that address.
    let refused = save(panel.state.config.public_url.clone()).await?;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

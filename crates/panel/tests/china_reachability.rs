#![forbid(unsafe_code)]
//! The administrator boundary of the China reachability monitor. Nothing here
//! reaches Globalping: `/run` only queues, and no worker runs in the test panel.
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result, ensure};
use apsp_panel::auth::{hash_token, random_token};
use apsp_protocol::now_timestamp;
use business_support::TestPanel;
use reqwest::{Method, StatusCode, header};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const ROOT: &str = "/api/plugins/sing-box/reachability";
const TOKEN: &str = "fixture-globalping-token-value";

async fn prove(panel: &TestPanel, cookie: &str) -> Result<()> {
    let hash = hash_token(cookie.split_once('=').context("cookie")?.1);
    sqlx::query("INSERT INTO administrator_reauth(session_hash,verified_at,expires_at) VALUES($1,$2,$3) ON CONFLICT(session_hash) DO UPDATE SET verified_at=EXCLUDED.verified_at,expires_at=EXCLUDED.expires_at").bind(hash).bind(now_timestamp()).bind(now_timestamp()+3600).execute(&panel.state.pool).await?;
    Ok(())
}

async fn status_of(
    panel: &TestPanel,
    method: Method,
    path: &str,
    header_value: Option<(header::HeaderName, String)>,
    body: Option<Value>,
) -> Result<StatusCode> {
    let mut request = panel
        .client
        .request(method, format!("{}{path}", panel.base))
        .header("Origin", &panel.state.config.public_url);
    if let Some((name, value)) = header_value {
        request = request.header(name, value);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    Ok(request.send().await?.status())
}

fn routes() -> Vec<(Method, String, Option<Value>)> {
    vec![
        (Method::GET, ROOT.to_owned(), None),
        (Method::GET, format!("{ROOT}/settings"), None),
        (
            Method::PUT,
            format!("{ROOT}/settings"),
            Some(json!({"enabled":true,"interval_minutes":30,"revision":1})),
        ),
        (Method::POST, format!("{ROOT}/run"), None),
    ]
}

async fn proxy_member(panel: &TestPanel) -> Result<(i64, Uuid, String)> {
    let pool = &panel.state.pool;
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(name,subscription_token) VALUES('fixture member',$1) RETURNING id",
    )
    .bind(random_token())
    .fetch_one(pool)
    .await?;
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
    Ok((user, account, format!("apsp_proxy_session={token}")))
}

#[sqlx::test]
async fn only_an_owner_session_reaches_the_monitor(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let (_, _, member_cookie) = proxy_member(&panel).await?;

    // A restricted administrator with the proxy capability still is not the owner.
    let operator: i64 = sqlx::query_scalar("INSERT INTO admins(password_hash) SELECT password_hash FROM admins WHERE id=1 RETURNING id").fetch_one(&pool).await?;
    sqlx::query("INSERT INTO administrator_profiles(admin_id,login_name,display_name,role,all_servers,capabilities,created_at,updated_at) VALUES($1,'operator','运维','operator',true,'[\"proxy:read\",\"proxy:write\",\"monitoring:read\"]',0,0)").bind(operator).execute(&pool).await?;
    let session = random_token();
    sqlx::query("INSERT INTO sessions(token_hash,admin_id,expires_at) VALUES($1,$2,$3)")
        .bind(hash_token(&session))
        .bind(operator)
        .bind(now_timestamp() + 3600)
        .execute(&pool)
        .await?;
    let operator_cookie = format!("apsp_session={session}");
    prove(&panel, &operator_cookie).await?;

    // An API token of the owner cannot act as the owner's session.
    let api_token = format!("apsp_api_{}", random_token());
    sqlx::query("INSERT INTO management_api_tokens(id,admin_id,token_hash,name,capabilities,server_ids,all_servers,expires_at,created_at) VALUES($1,1,$2,'proxy','[\"proxy:read\",\"proxy:write\"]','[]',true,$3,0)")
        .bind(Uuid::new_v4())
        .bind(hash_token(&api_token))
        .bind(now_timestamp() + 3600)
        .execute(&pool)
        .await?;

    for (method, path, body) in routes() {
        assert_eq!(
            status_of(&panel, method.clone(), &path, None, body.clone()).await?,
            StatusCode::UNAUTHORIZED,
            "anonymous {method} {path}"
        );
        assert_eq!(
            status_of(
                &panel,
                method.clone(),
                &path,
                Some((header::COOKIE, member_cookie.clone())),
                body.clone()
            )
            .await?,
            StatusCode::UNAUTHORIZED,
            "member {method} {path}"
        );
        assert_eq!(
            status_of(
                &panel,
                method.clone(),
                &path,
                Some((header::COOKIE, operator_cookie.clone())),
                body.clone()
            )
            .await?,
            StatusCode::FORBIDDEN,
            "operator {method} {path}"
        );
        assert_eq!(
            status_of(
                &panel,
                method.clone(),
                &path,
                Some((header::AUTHORIZATION, format!("Bearer {api_token}"))),
                body.clone()
            )
            .await?,
            StatusCode::FORBIDDEN,
            "API token {method} {path}"
        );
    }
    // None of the refused requests changed anything.
    let (enabled, revision, requested): (bool, i64, Option<i64>) = sqlx::query_as(
        "SELECT enabled,revision,run_requested_at FROM singbox_reachability_settings",
    )
    .fetch_one(&pool)
    .await?;
    assert!(!enabled);
    assert_eq!(revision, 1);
    assert_eq!(requested, None);
    Ok(())
}

#[sqlx::test]
async fn opting_in_needs_a_recent_proof_and_a_run_needs_the_opt_in(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;

    let settings: Value = panel
        .admin(Method::GET, &format!("{ROOT}/settings"), &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(settings["enabled"], false);
    assert_eq!(settings["interval_minutes"], 30);
    assert_eq!(settings["minimum_interval_minutes"], 15);
    assert_eq!(settings["token_configured"], false);

    // Disabled by default: nothing may be queued toward the third party.
    let refused = panel
        .admin(Method::POST, &format!("{ROOT}/run"), &cookie, None)
        .await?;
    assert_eq!(refused.status(), StatusCode::CONFLICT);

    // Logging in counts as a proof; once it has lapsed, opting in must ask again.
    sqlx::query("DELETE FROM administrator_reauth")
        .execute(&pool)
        .await?;
    let opt_in = json!({"enabled":true,"interval_minutes":30,"revision":1});
    let unproven = panel
        .admin(
            Method::PUT,
            &format!("{ROOT}/settings"),
            &cookie,
            Some(opt_in.clone()),
        )
        .await?;
    assert_eq!(unproven.status(), StatusCode::FORBIDDEN);
    let (enabled, revision): (bool, i64) =
        sqlx::query_as("SELECT enabled,revision FROM singbox_reachability_settings")
            .fetch_one(&pool)
            .await?;
    assert!(!enabled, "a refused opt-in must change nothing");
    assert_eq!(revision, 1);
    prove(&panel, &cookie).await?;

    for (body, status) in [
        (
            json!({"enabled":true,"interval_minutes":14,"revision":1}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"enabled":true,"interval_minutes":1441,"revision":1}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"enabled":true,"interval_minutes":30,"revision":99}),
            StatusCode::CONFLICT,
        ),
        (
            json!({"enabled":true,"interval_minutes":30,"revision":1,"api_token":"has space"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"enabled":true,"interval_minutes":30,"revision":1,"api_token":"a","clear_token":true}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = panel
            .admin(
                Method::PUT,
                &format!("{ROOT}/settings"),
                &cookie,
                Some(body.clone()),
            )
            .await?;
        assert_eq!(response.status(), status, "{body}");
    }

    let saved: Value = panel
        .admin(
            Method::PUT,
            &format!("{ROOT}/settings"),
            &cookie,
            Some(opt_in),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(saved["enabled"], true);
    assert_eq!(saved["revision"], 2);
    let queued: Value = panel
        .admin(Method::POST, &format!("{ROOT}/run"), &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(queued["queued"], true);
    let requested: Option<i64> =
        sqlx::query_scalar("SELECT run_requested_at FROM singbox_reachability_settings")
            .fetch_one(&pool)
            .await?;
    assert!(requested.is_some());

    // Opting out also drops a queued request.
    let off: Value = panel
        .admin(
            Method::PUT,
            &format!("{ROOT}/settings"),
            &cookie,
            Some(json!({"enabled":false,"interval_minutes":60,"revision":2})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(off["enabled"], false);
    assert_eq!(off["interval_minutes"], 60);
    assert_eq!(off["run_requested"], false);
    Ok(())
}

#[sqlx::test]
async fn the_api_token_is_encrypted_and_never_returned(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    prove(&panel, &cookie).await?;
    let response = panel
        .admin(
            Method::PUT,
            &format!("{ROOT}/settings"),
            &cookie,
            Some(json!({"enabled":false,"interval_minutes":30,"revision":1,"api_token":TOKEN})),
        )
        .await?;
    let status = response.status();
    let body = response.text().await?;
    ensure!(!body.contains(TOKEN), "response exposed the token");
    ensure!(status.is_success(), "{status}: {body}");
    let saved: Value = serde_json::from_str(&body)?;
    assert_eq!(saved["token_configured"], true);
    assert!(saved.get("credential_id").is_none());

    let (credential, kind, ciphertext): (Uuid, String, Vec<u8>) = sqlx::query_as(
        "SELECT c.id,c.kind,c.ciphertext FROM singbox_reachability_settings s JOIN credential_entries c ON c.id=s.credential_id",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(kind, "external-api");
    assert!(
        !ciphertext
            .windows(TOKEN.len())
            .any(|window| window == TOKEN.as_bytes())
    );
    // The guard writes every mutation to the audit log; the token must be redacted there.
    let audit: String = sqlx::query_scalar(
        "SELECT string_agg(request_diff::text,' ') FROM management_audit WHERE object_path LIKE '%/reachability/settings'",
    )
    .fetch_one(&pool)
    .await?;
    assert!(!audit.contains(TOKEN));
    for path in [ROOT.to_owned(), format!("{ROOT}/settings")] {
        let read = panel
            .admin(Method::GET, &path, &cookie, None)
            .await?
            .text()
            .await?;
        assert!(!read.contains(TOKEN));
    }

    // Keeping the token needs no field; replacing or clearing retires the old entry.
    let kept: Value = panel
        .admin(
            Method::PUT,
            &format!("{ROOT}/settings"),
            &cookie,
            Some(json!({"enabled":false,"interval_minutes":45,"revision":2})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(kept["token_configured"], true);
    let cleared: Value = panel
        .admin(
            Method::PUT,
            &format!("{ROOT}/settings"),
            &cookie,
            Some(json!({"enabled":false,"interval_minutes":45,"revision":3,"clear_token":true})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(cleared["token_configured"], false);
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM credential_entries WHERE id=$1")
        .bind(credential)
        .fetch_one(&pool)
        .await?;
    assert!(!enabled);
    Ok(())
}

#[sqlx::test]
async fn the_owner_sees_flags_and_no_member_or_subscription_response_mentions_the_monitor(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server: i64 =
        sqlx::query_scalar("INSERT INTO servers(name) VALUES('fixture') RETURNING id")
            .fetch_one(&pool)
            .await?;
    sqlx::query("INSERT INTO nodes(name,server_id,port,public_host,sni,private_key,public_key,short_id) VALUES('fixture node',$1,8443,'node.example.com','www.apple.com','fixture-private','fixture-public','0123abcd')")
        .bind(server)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO singbox_reachability_targets(host,port,state,seen_at,checked_at,detail) VALUES('node.example.com',8443,'ip_blocked',$1,$1,$2)")
        .bind(now_timestamp())
        .bind(json!({"china":{"probes":[{"carrier":"telecom","network":"China Telecom","city":"Fixture","connected":false,"avg_ms":null}],"error":null}}))
        .execute(&pool)
        .await?;

    let body = panel
        .admin(Method::GET, ROOT, &cookie, None)
        .await?
        .text()
        .await?;
    assert!(!body.contains("fixture-private"));
    let listed: Value = serde_json::from_str(&body)?;
    let row = &listed["targets"][0];
    assert_eq!(row["key"], "node.example.com:8443");
    assert_eq!(row["state"], "ip_blocked");
    assert_eq!(row["blocked"], true);
    assert_eq!(row["nodes"][0]["name"], "fixture node");
    assert_eq!(row["detail"]["china"]["probes"][0]["carrier"], "telecom");
    let codes: Vec<&str> = row["risks"]
        .as_array()
        .context("risks")?
        .iter()
        .filter_map(|risk| risk["code"].as_str())
        .collect();
    assert_eq!(codes, ["name_risky_brand", "port_not_443"]);

    let (user, account, member_cookie) = proxy_member(&panel).await?;
    let token: String = sqlx::query_scalar("SELECT subscription_token FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(&pool)
        .await?;
    for (path, member) in [
        (format!("/sub/{token}"), None),
        (
            format!("/api/plugins/sing-box/portal/{account}"),
            Some(member_cookie.clone()),
        ),
        (
            format!("/api/plugins/sing-box/portal/{account}/commerce/messages"),
            Some(member_cookie.clone()),
        ),
    ] {
        let mut request = panel.client.get(format!("{}{path}", panel.base));
        if let Some(member) = member {
            request = request.header(header::COOKIE, member);
        }
        let text = request.send().await?.text().await?;
        for leaked in ["reachab", "ip_blocked", "port_blocked", "globalping"] {
            ensure!(!text.contains(leaked), "{path} mentioned {leaked}");
        }
    }
    Ok(())
}

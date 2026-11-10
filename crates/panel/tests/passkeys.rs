#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use business_support::TestPanel;
use reqwest::{Method, StatusCode, header};
use serde_json::{Value, json};
use sqlx::PgPool;

const PASSWORD: &str = "business-test-password";

#[sqlx::test]
async fn reauthentication_requires_a_session_exact_origin_and_existing_keys(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    sqlx::query("DELETE FROM administrator_reauth")
        .execute(&pool)
        .await?;
    let path = "/api/control-center/reauth/passkey/start";
    assert_eq!(
        post(&panel, "", path, json!({})).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        panel
            .admin(Method::POST, path, &cookie, None)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let response = post(&panel, &cookie, path, json!({})).await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: Value = response.json().await?;
    assert_eq!(error["error"], "此账户尚未绑定 Passkey。");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM administrator_reauth")
            .fetch_one(&pool)
            .await?,
        0
    );
    sqlx::query(
        "INSERT INTO admins(id,password_hash) SELECT 2,password_hash FROM admins WHERE id=1",
    )
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO administrator_profiles(admin_id,login_name,display_name,role,all_servers,created_at,updated_at) VALUES(2,'other-owner','其他所有者','owner',true,0,0)")
        .execute(&pool).await?;
    sqlx::query("UPDATE sessions SET admin_id=2 WHERE admin_id=1")
        .execute(&pool)
        .await?;
    // Other administrators re-authenticate with their own keys; this one has none.
    let response = post(&panel, &cookie, path, json!({})).await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: Value = response.json().await?;
    assert_eq!(error["error"], "此账户尚未绑定 Passkey。");
    Ok(())
}

/// Ends every recent proof, as five minutes passing would.
async fn expire_proofs(pool: &PgPool) -> Result<()> {
    sqlx::query("UPDATE administrator_reauth SET expires_at=0")
        .execute(pool)
        .await?;
    Ok(())
}

async fn reauth(panel: &TestPanel, cookie: &str, password: &str) -> Result<StatusCode> {
    Ok(post(
        panel,
        cookie,
        "/api/control-center/reauth",
        json!({"password": password}),
    )
    .await?
    .status())
}

async fn post(
    panel: &TestPanel,
    cookie: &str,
    path: &str,
    body: Value,
) -> Result<reqwest::Response> {
    Ok(panel
        .client
        .post(format!("{}{path}", panel.base))
        .header(header::ORIGIN, &panel.state.config.public_url)
        .header(header::COOKIE, cookie)
        .json(&body)
        .send()
        .await?)
}

#[sqlx::test]
async fn insecure_origins_disable_passkeys_without_breaking_password_login(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let status: Value = panel
        .admin(Method::GET, "/api/security/passkeys", &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(status["configuration"]["enabled"], false);
    assert!(
        status["configuration"]["reason"]
            .as_str()
            .unwrap()
            .contains("HTTPS")
    );
    assert_eq!(
        post(&panel, &cookie, "/api/login/passkey/start", json!({}))
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        panel
            .admin(Method::GET, "/api/me", &cookie, None)
            .await?
            .status(),
        StatusCode::OK
    );
    Ok(())
}

#[sqlx::test]
async fn registration_requires_fresh_admin_proof_exact_origin_and_a_bound_single_use_challenge(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let path = "/api/security/passkeys/register/start";
    let input = json!({"name":"测试密钥"});
    assert_eq!(
        post(&panel, "", path, input.clone()).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        panel
            .admin(Method::POST, path, &cookie, Some(input.clone()))
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        panel
            .client
            .post(format!("{}{path}", panel.base))
            .header(header::COOKIE, &cookie)
            .header(header::ORIGIN, "https://other.example.com")
            .json(&input)
            .send()
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    // Registration needs the shared recent proof (password or Passkey); the
    // request itself no longer carries a password.
    expire_proofs(&pool).await?;
    assert_eq!(
        post(&panel, &cookie, path, input.clone()).await?.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        reauth(&panel, &cookie, "wrong").await?,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post(&panel, &cookie, path, input.clone()).await?.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(reauth(&panel, &cookie, PASSWORD).await?, StatusCode::OK);
    assert_eq!(
        post(
            &panel,
            &cookie,
            path,
            json!({"name":"测试","password":PASSWORD})
        )
        .await?
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let response = post(&panel, &cookie, path, input)
        .await?
        .error_for_status()?;
    let binding = response.headers()[header::SET_COOKIE]
        .to_str()?
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert!(
        response.headers()[header::SET_COOKIE]
            .to_str()?
            .contains("HttpOnly")
    );
    let challenge: Value = response.json().await?;
    assert!(challenge.get("state").is_none());
    assert_eq!(
        challenge["options"]["publicKey"]["authenticatorSelection"]["userVerification"],
        "required"
    );
    let credential = json!({"id":"AA","rawId":"AA","type":"public-key","response":{"attestationObject":"AA","clientDataJSON":"AA"}});
    let finish = json!({"challenge_id":challenge["challenge_id"],"credential":credential});
    let finish_path = "/api/security/passkeys/register/finish";
    assert_eq!(
        post(&panel, &cookie, finish_path, finish.clone())
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM passkey_ceremonies")
            .fetch_one(&pool)
            .await?,
        1
    );
    assert_eq!(
        post(
            &panel,
            &format!("{cookie}; {binding}"),
            finish_path,
            finish.clone()
        )
        .await?
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM passkey_ceremonies")
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        post(&panel, &format!("{cookie}; {binding}"), finish_path, finish)
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    Ok(())
}

#[sqlx::test]
async fn proxy_invitations_are_scoped_hashed_rotatable_and_removed_with_the_owner(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let user = panel.create_user(&cookie, "受验代理用户").await?;
    let id = user["id"].as_i64().unwrap();
    let path = format!("/api/plugins/sing-box/users/{id}/portal");
    assert_eq!(
        post(&panel, "", &format!("{path}/invitation"), json!({}))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    // Issuing a link needs the shared recent proof, not a password in the request.
    expire_proofs(&pool).await?;
    assert_eq!(
        post(&panel, &cookie, &format!("{path}/invitation"), json!({}))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(reauth(&panel, &cookie, PASSWORD).await?, StatusCode::OK);
    // The link still comes only from the configured origin.
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &format!("{path}/invitation"),
                &cookie,
                Some(json!({}))
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post(
            &panel,
            &cookie,
            &format!("{path}/invitation"),
            json!({"password":PASSWORD})
        )
        .await?
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let first: Value = post(&panel, &cookie, &format!("{path}/invitation"), json!({}))
        .await?
        .error_for_status()?
        .json()
        .await?;
    let url = first["url"].as_str().unwrap();
    let (base, token) = url.split_once("?activate=").unwrap();
    assert!(base.contains("/#/account/"));
    let account = base.rsplit('/').next().unwrap();
    let saved: String =
        sqlx::query_scalar("SELECT activation_hash FROM singbox_portal_accounts WHERE user_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(saved, apsp_panel_host::auth::hash_token(token));
    let view: Value = panel
        .admin(Method::GET, &path, &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(!view.to_string().contains(token));
    let own_path = format!("/api/plugins/sing-box/portal/{account}");
    let anonymous: Value = panel
        .admin(Method::GET, &own_path, &cookie, None)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        anonymous["authenticated"], false,
        "An administrator cookie cannot become a proxy session"
    );
    assert!(anonymous.get("name").is_none());
    assert_eq!(
        post(
            &panel,
            "",
            &format!("{own_path}/register/start"),
            json!({"name":"密钥","activation_token":user["subscription_token"]})
        )
        .await?
        .status(),
        StatusCode::BAD_REQUEST
    );
    let second: Value = post(&panel, &cookie, &format!("{path}/invitation"), json!({}))
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_ne!(second["url"], first["url"]);
    assert_eq!(
        post(
            &panel,
            "",
            &format!("{own_path}/register/start"),
            json!({"name":"密钥","activation_token":token})
        )
        .await?
        .status(),
        StatusCode::BAD_REQUEST
    );
    let new_token = second["url"]
        .as_str()
        .unwrap()
        .split_once("?activate=")
        .unwrap()
        .1;
    post(
        &panel,
        "",
        &format!("{own_path}/register/start"),
        json!({"name":"密钥","activation_token":new_token}),
    )
    .await?
    .error_for_status()?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM passkey_ceremonies")
            .fetch_one(&pool)
            .await?,
        1
    );
    panel
        .admin(
            Method::DELETE,
            &format!("/api/plugins/sing-box/users/{id}"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM singbox_portal_accounts")
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM passkey_ceremonies")
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        panel
            .admin(Method::GET, &own_path, "", None)
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[sqlx::test]
async fn passkey_requests_share_the_login_rate_limit_and_fail_closed_on_large_bodies(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    for _ in 0..7 {
        assert_eq!(
            post(&panel, "", "/api/login/passkey/start", json!({}))
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        post(&panel, "", "/api/login/passkey/start", json!({}))
            .await?
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        post(
            &panel,
            &cookie,
            "/api/security/passkeys/register/start",
            json!({"name":"a".repeat(70_000)})
        )
        .await?
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    Ok(())
}

#[sqlx::test]
#[ignore = "requires Bun, Playwright and Chromium with a virtual WebAuthn authenticator; run explicitly against the isolated test database"]
async fn virtual_authenticator_browser_roundtrip(pool: PgPool) -> Result<()> {
    use sqlx::ConnectOptions;
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let mut command = tokio::process::Command::new("bun");
    command
        .arg("web/tests/passkeys.mjs")
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .env("APSP_PASSKEY_TEST_ORIGIN", &panel.state.config.public_url)
        .env(
            "APSP_PASSKEY_TEST_DATABASE_URL",
            pool.connect_options().to_url_lossy().as_str(),
        )
        .kill_on_drop(true);
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(240), command.output()).await??;
    anyhow::ensure!(
        result.status.success(),
        "browser test failed: {}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    println!("{}", String::from_utf8_lossy(&result.stdout));
    Ok(())
}

/// Signs in a fixture administrator created with the owner's password hash.
async fn named_cookie(panel: &TestPanel, login_name: &str) -> Result<String> {
    let response = panel
        .client
        .post(format!("{}/api/login", panel.base))
        .json(&json!({"login_name":login_name,"password":PASSWORD}))
        .send()
        .await?
        .error_for_status()?;
    Ok(response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .unwrap_or_default()
        .to_owned())
}

#[sqlx::test]
async fn named_administrators_have_their_own_passkey_accounts(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start_with_localhost(pool.clone()).await?;
    let owner = panel.admin_cookie().await?;
    sqlx::query(
        "INSERT INTO admins(id,password_hash) SELECT 7,password_hash FROM admins WHERE id=1",
    )
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO administrator_profiles(admin_id,login_name,display_name,role,all_servers,capabilities,created_at,updated_at) VALUES(7,'operator','运维值班','viewer',true,'[\"servers:read\"]',0,0)")
        .execute(&pool).await?;
    let operator = named_cookie(&panel, "operator").await?;
    // A viewer without security rights manages the keys of their own account.
    let list = panel
        .admin(Method::GET, "/api/security/passkeys", &operator, None)
        .await?;
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(list.json::<Value>().await?["keys"], json!([]));
    let start = "/api/security/passkeys/register/start";
    // Each administrator proves their own identity through the shared proof.
    expire_proofs(&pool).await?;
    let named = json!({"name":"值班密钥"});
    assert_eq!(
        post(&panel, &operator, start, named.clone())
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        reauth(&panel, &operator, "wrong-password").await?,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(reauth(&panel, &operator, PASSWORD).await?, StatusCode::OK);
    let response = post(&panel, &operator, start, named).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let options: Value = response.json().await?;
    assert_eq!(options["options"]["publicKey"]["user"]["name"], "operator");
    assert_eq!(
        options["options"]["publicKey"]["user"]["displayName"],
        "运维值班"
    );
    // A fresh database has no owner row when migrations run; the owner gets the
    // original account on their first registration, as upgraded panels already have.
    assert_eq!(reauth(&panel, &owner, PASSWORD).await?, StatusCode::OK);
    assert_eq!(
        post(&panel, &owner, start, json!({"name":"所有者密钥"}))
            .await?
            .status(),
        StatusCode::OK
    );
    let accounts: Vec<(i64, uuid::Uuid)> = sqlx::query_as(
        "SELECT admin_id,account_id FROM administrator_passkey_accounts ORDER BY admin_id",
    )
    .fetch_all(&pool)
    .await?;
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0], (1, uuid::Uuid::from_u128(1)));
    assert_eq!(accounts[1].0, 7);
    assert_ne!(accounts[1].1, uuid::Uuid::from_u128(1));
    // The owner's keys stay separate.
    let owner_list = panel
        .admin(Method::GET, "/api/security/passkeys", &owner, None)
        .await?;
    assert_eq!(owner_list.json::<Value>().await?["keys"], json!([]));
    // API tokens cannot manage anyone's passkeys.
    let token = "apsp_api_named_passkey_token";
    sqlx::query("INSERT INTO management_api_tokens(id,admin_id,token_hash,name,capabilities,server_ids,all_servers,expires_at,created_at) VALUES($1,7,$2,'viewer','[\"servers:read\"]','[]',true,$3,0)")
        .bind(uuid::Uuid::new_v4()).bind(apsp_panel_host::auth::hash_token(token)).bind(apsp_protocol::now_timestamp() + 3600).execute(&pool).await?;
    assert_eq!(
        panel
            .client
            .get(format!("{}/api/security/passkeys", panel.base))
            .bearer_auth(token)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    // Sign-in names the administrator. Unknown names, names without keys and
    // the default owner without keys all get the same answer.
    let login = "/api/login/passkey/start";
    let mut answers = Vec::new();
    for body in [
        Some(json!({"login_name":"operator"})),
        Some(json!({"login_name":"nobody"})),
        None,
    ] {
        let request = panel
            .client
            .post(format!("{}{login}", panel.base))
            .header(header::ORIGIN, &panel.state.config.public_url);
        let response = match body {
            Some(body) => request.json(&body).send().await?,
            None => request.send().await?,
        };
        answers.push((response.status(), response.text().await?));
    }
    assert_eq!(answers[0].0, StatusCode::BAD_REQUEST);
    assert!(answers.iter().all(|answer| answer == &answers[0]));
    // The attempts above share the login rate limit with password sign-in.
    sqlx::query("UPDATE auth_rate_limits SET attempts=0,window_start=0")
        .execute(&pool)
        .await?;
    let malformed = panel
        .client
        .post(format!("{}{login}", panel.base))
        .header(header::ORIGIN, &panel.state.config.public_url)
        .header(header::CONTENT_TYPE, "application/json")
        .body("{")
        .send()
        .await?;
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        malformed.json::<Value>().await?["error"],
        "登录请求格式无效。"
    );
    Ok(())
}

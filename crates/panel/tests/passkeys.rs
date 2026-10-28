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
    assert_eq!(
        post(&panel, &cookie, path, json!({})).await?.status(),
        StatusCode::FORBIDDEN
    );
    Ok(())
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
    let input = json!({"name":"测试密钥","password":PASSWORD});
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
    assert_eq!(
        post(
            &panel,
            &cookie,
            path,
            json!({"name":"测试","password":"wrong"})
        )
        .await?
        .status(),
        StatusCode::BAD_REQUEST
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
        post(
            &panel,
            "",
            &format!("{path}/invitation"),
            json!({"password":PASSWORD})
        )
        .await?
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let first: Value = post(
        &panel,
        &cookie,
        &format!("{path}/invitation"),
        json!({"password":PASSWORD}),
    )
    .await?
    .error_for_status()?
    .json()
    .await?;
    let url = first["url"].as_str().unwrap();
    let (base, token) = url.split_once("?activate=").unwrap();
    assert!(base.contains("/#/plugins/sing-box/account/"));
    let account = base.rsplit('/').next().unwrap();
    let saved: String =
        sqlx::query_scalar("SELECT activation_hash FROM singbox_portal_accounts WHERE user_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(saved, apsp_panel::auth::hash_token(token));
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
    let second: Value = post(
        &panel,
        &cookie,
        &format!("{path}/invitation"),
        json!({"password":PASSWORD}),
    )
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
            json!({"name":"a".repeat(70_000),"password":PASSWORD})
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

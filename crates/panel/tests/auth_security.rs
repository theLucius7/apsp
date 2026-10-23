#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use apsp_panel::{AppState, config::Config, router};
use apsp_protocol::now_timestamp;
use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use reqwest::{Client, Response, StatusCode, header};
use serde_json::{Value, json};
use sha1::Sha1;
use sqlx::PgPool;
use std::{net::SocketAddr, path::PathBuf, time::Duration};
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;

const PASSWORD: &str = "security-fixture-password";
const SECRET: &[u8] = b"12345678901234567890";

struct Panel {
    pool: PgPool,
    permits: std::sync::Arc<tokio::sync::Semaphore>,
    client: Client,
    base: String,
    directory: PathBuf,
    task: JoinHandle<()>,
}

impl Panel {
    async fn start(pool: PgPool) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let base = format!("http://{address}");
        let directory = std::env::temp_dir().join(format!("apsp-security-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory)?;
        let config = Config {
            database_url: String::new(),
            listen: address,
            public_url: base.clone(),
            data_dir: directory.clone(),
            admin_password: Some(PASSWORD.into()),
        };
        let state = AppState::new(pool.clone(), config).await?;
        let permits = state.login_permits.clone();
        let app = router(state).into_make_service_with_connect_info::<SocketAddr>();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("test HTTP server");
        });
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(15))
            .build()?;
        Ok(Self {
            pool,
            permits,
            client,
            base,
            directory,
            task,
        })
    }

    async fn login(&self, password: &str, code: Option<&str>) -> Result<Response> {
        Ok(self
            .client
            .post(format!("{}/api/login", self.base))
            .json(&json!({"password": password, "totp_code": code}))
            .send()
            .await?)
    }

    async fn post(&self, path: &str, cookie: &str, body: Value) -> Result<Response> {
        Ok(self
            .client
            .post(format!("{}{path}", self.base))
            .header(header::COOKIE, cookie)
            .json(&body)
            .send()
            .await?)
    }

    async fn get(&self, path: &str, cookie: &str) -> Result<Response> {
        Ok(self
            .client
            .get(format!("{}{path}", self.base))
            .header(header::COOKIE, cookie)
            .send()
            .await?)
    }

    async fn expire_rate_window(&self) -> Result<()> {
        sqlx::query("UPDATE auth_rate_limits SET window_start = $1")
            .bind(now_timestamp() - 61)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn cookie(response: &Response) -> Result<String> {
    assert_eq!(response.status(), StatusCode::OK);
    Ok(response
        .headers()
        .get(header::SET_COOKIE)
        .context("session cookie")?
        .to_str()?
        .split(';')
        .next()
        .context("cookie value")?
        .to_string())
}

fn code(secret: &[u8], step: i64) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC key");
    mac.update(&(step as u64).to_be_bytes());
    let result = mac.finalize().into_bytes();
    let offset = (result[19] & 15) as usize;
    let binary = u32::from_be_bytes(result[offset..offset + 4].try_into().expect("four bytes"))
        & 0x7fff_ffff;
    format!("{:06}", binary % 1_000_000)
}

#[sqlx::test]
async fn setup_confirm_disable_require_factors_and_revoke_other_sessions(
    pool: PgPool,
) -> Result<()> {
    let panel = Panel::start(pool).await?;
    let first = cookie(&panel.login(PASSWORD, None).await?)?;
    let second = cookie(&panel.login(PASSWORD, None).await?)?;
    assert_eq!(
        panel
            .post(
                "/api/security/totp/setup",
                &first,
                json!({"password":"wrong"})
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let response = panel
        .post(
            "/api/security/totp/setup",
            &first,
            json!({"password":PASSWORD}),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let setup: Value = response.json().await?;
    let secret = BASE32_NOPAD.decode(setup["secret"].as_str().context("seed")?.as_bytes())?;
    assert_eq!(secret.len(), 20);
    assert!(
        setup["otpauth_uri"]
            .as_str()
            .context("URI")?
            .starts_with("otpauth://totp/Apsp:admin?secret=")
    );
    let status: Value = panel
        .get("/api/security/totp", &first)
        .await?
        .json()
        .await?;
    assert_eq!(status["enabled"], false);
    assert!(status.get("secret").is_none() && status.get("otpauth_uri").is_none());
    let confirm = code(&secret, now_timestamp() / 30 - 1);
    assert_eq!(
        panel
            .post(
                "/api/security/totp/confirm",
                &second,
                json!({"code":confirm})
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        panel
            .post(
                "/api/security/totp/confirm",
                &first,
                json!({"code":"invalid"})
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        panel
            .post(
                "/api/security/totp/confirm",
                &first,
                json!({"code":confirm})
            )
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        panel.get("/api/me", &second).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(panel.get("/api/me", &first).await?.status(), StatusCode::OK);
    assert_eq!(
        panel.login(PASSWORD, None).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    panel.expire_rate_window().await?;
    let current = code(&secret, now_timestamp() / 30);
    let third = cookie(&panel.login(PASSWORD, Some(&current)).await?)?;
    let future = code(&secret, now_timestamp() / 30 + 1);
    assert_eq!(
        panel
            .post(
                "/api/security/totp/disable",
                &first,
                json!({"password":"wrong","code":future})
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        panel
            .post(
                "/api/security/totp/disable",
                &first,
                json!({"password":PASSWORD,"code":""})
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        panel
            .post(
                "/api/security/totp/disable",
                &first,
                json!({"password":PASSWORD,"code":future})
            )
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        panel.get("/api/me", &third).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(panel.get("/api/me", &first).await?.status(), StatusCode::OK);
    assert_eq!(panel.login(PASSWORD, None).await?.status(), StatusCode::OK);
    let cleared: bool = sqlx::query_scalar("SELECT totp_secret IS NULL AND totp_last_step IS NULL AND totp_pending_secret IS NULL AND totp_pending_session IS NULL FROM admins WHERE id=1")
        .fetch_one(&panel.pool).await?;
    assert!(cleared);
    Ok(())
}

#[sqlx::test]
async fn totp_login_rejects_missing_wrong_and_concurrent_replayed_codes(
    pool: PgPool,
) -> Result<()> {
    let panel = Panel::start(pool).await?;
    sqlx::query("UPDATE admins SET totp_secret = $1 WHERE id=1")
        .bind(SECRET)
        .execute(&panel.pool)
        .await?;
    let valid = code(SECRET, now_timestamp() / 30);
    assert_eq!(
        panel.login(PASSWORD, None).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        panel.login(PASSWORD, Some("invalid")).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        panel.login("wrong", Some(&valid)).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    let (left, right) = tokio::join!(
        panel.login(PASSWORD, Some(&valid)),
        panel.login(PASSWORD, Some(&valid))
    );
    let mut codes = [left?.status().as_u16(), right?.status().as_u16()];
    codes.sort();
    assert_eq!(codes, [200, 401]);
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE admin_id=1")
        .fetch_one(&panel.pool)
        .await?;
    assert_eq!(sessions, 1);
    assert_eq!(
        panel.login(PASSWORD, Some(&valid)).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[sqlx::test]
async fn setup_expiration_and_replacement_do_not_enable_or_disclose_seed(
    pool: PgPool,
) -> Result<()> {
    let panel = Panel::start(pool).await?;
    assert_eq!(
        panel.get("/api/security/totp", "").await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        panel
            .post("/api/security/totp/setup", "", json!({"password":PASSWORD}))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let session = cookie(&panel.login(PASSWORD, None).await?)?;
    let first: Value = panel
        .post(
            "/api/security/totp/setup",
            &session,
            json!({"password":PASSWORD}),
        )
        .await?
        .json()
        .await?;
    let second: Value = panel
        .post(
            "/api/security/totp/setup",
            &session,
            json!({"password":PASSWORD}),
        )
        .await?
        .json()
        .await?;
    assert_ne!(first["secret"], second["secret"]);
    let secret = BASE32_NOPAD.decode(second["secret"].as_str().context("seed")?.as_bytes())?;
    sqlx::query("UPDATE admins SET totp_pending_expires = $1 WHERE id=1")
        .bind(now_timestamp() - 1)
        .execute(&panel.pool)
        .await?;
    let value = code(&secret, now_timestamp() / 30);
    assert_eq!(
        panel
            .post(
                "/api/security/totp/confirm",
                &session,
                json!({"code":value})
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let status: Value = panel
        .get("/api/security/totp", &session)
        .await?
        .json()
        .await?;
    assert_eq!(status, json!({"enabled":false,"pending_expires_at":null}));
    assert_eq!(panel.login(PASSWORD, None).await?.status(), StatusCode::OK);
    Ok(())
}

#[sqlx::test]
async fn peer_rate_limit_ignores_forwarded_headers_and_survives_restart(
    pool: PgPool,
) -> Result<()> {
    let panel = Panel::start(pool.clone()).await?;
    for index in 0..8 {
        let response = panel
            .client
            .post(format!("{}/api/login", panel.base))
            .header("X-Forwarded-For", format!("192.0.2.{}", index + 1))
            .header("Forwarded", format!("for=192.0.2.{}", index + 1))
            .json(&json!({"password":""}))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        panel.login(PASSWORD, None).await?.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth_rate_limits WHERE scope <> 'global'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(rows, 1);
    drop(panel);
    let restarted = Panel::start(pool).await?;
    assert_eq!(
        restarted.login(PASSWORD, None).await?.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    restarted.expire_rate_window().await?;
    assert_eq!(
        restarted.login(PASSWORD, None).await?.status(),
        StatusCode::OK
    );
    Ok(())
}

#[sqlx::test]
async fn global_budget_and_hashing_concurrency_are_enforced(pool: PgPool) -> Result<()> {
    let panel = Panel::start(pool.clone()).await?;
    sqlx::query("UPDATE auth_rate_limits SET window_start=$1, attempts=64 WHERE scope='global'")
        .bind(now_timestamp())
        .execute(&pool)
        .await?;
    assert_eq!(
        panel.login(PASSWORD, None).await?.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 0);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM auth_rate_limits")
        .fetch_one(&pool)
        .await?;
    assert_eq!(rows, 1);
    panel.expire_rate_window().await?;
    let occupied = panel.permits.acquire_many(4).await?;
    assert_eq!(
        panel.login(PASSWORD, None).await?.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(occupied);
    assert_eq!(panel.login(PASSWORD, None).await?.status(), StatusCode::OK);
    Ok(())
}

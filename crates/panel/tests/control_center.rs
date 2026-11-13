#![forbid(unsafe_code)]
use anyhow::{Context, Result};
use apsp_governance::auth::{hash_token, random_token};
use apsp_panel::router;
use apsp_panel_host::{
    config::Config,
    maintenance::{Cadence, Failure, periodic},
};
use apsp_protocol::now_timestamp;
use reqwest::{Client, StatusCode, header};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{net::SocketAddr, path::PathBuf, time::Duration};
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;

struct Panel {
    pool: PgPool,
    base: String,
    client: Client,
    directory: PathBuf,
    task: JoinHandle<()>,
}
impl Panel {
    async fn start(pool: PgPool) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let listen = listener.local_addr()?;
        let base = format!("http://{listen}");
        let directory = std::env::temp_dir().join(format!("apsp-control-test-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory)?;
        let state = apsp_panel::state(
            pool.clone(),
            Config {
                database_url: String::new(),
                listen,
                public_url: base.clone(),
                data_dir: directory.clone(),
                admin_password: Some("control-test-password".into()),
            },
        )
        .await?;
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                router(state).into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("test panel");
        });
        Ok(Self {
            pool,
            base,
            client: apsp_panel_host::http::Outbound::default().build()?,
            directory,
            task,
        })
    }
    async fn session(&self, actor: i64, recent: bool) -> Result<String> {
        let token = random_token();
        let hash = hash_token(&token);
        let now = now_timestamp();
        sqlx::query("INSERT INTO sessions(token_hash,admin_id,expires_at) VALUES($1,$2,$3)")
            .bind(&hash)
            .bind(actor)
            .bind(now + 3600)
            .execute(&self.pool)
            .await?;
        if recent {
            sqlx::query("INSERT INTO administrator_reauth(session_hash,verified_at,expires_at) VALUES($1,$2,$3)").bind(hash).bind(now).bind(now+300).execute(&self.pool).await?;
        }
        Ok(format!("apsp_session={token}"))
    }
    async fn server(&self, name: &str) -> Result<i64> {
        Ok(
            sqlx::query_scalar("INSERT INTO servers(name) VALUES($1) RETURNING id")
                .bind(name)
                .fetch_one(&self.pool)
                .await?,
        )
    }
    async fn viewer(&self, server: i64) -> Result<i64> {
        let actor:i64=sqlx::query_scalar("INSERT INTO admins(password_hash) SELECT password_hash FROM admins WHERE id=1 RETURNING id").fetch_one(&self.pool).await?;
        sqlx::query("INSERT INTO administrator_profiles(admin_id,login_name,display_name,role,all_servers,capabilities,created_at,updated_at) VALUES($1,'viewer','只读','viewer',false,'[\"servers:read\"]',0,0)").bind(actor).execute(&self.pool).await?;
        sqlx::query("INSERT INTO administrator_server_grants(admin_id,server_id) VALUES($1,$2)")
            .bind(actor)
            .bind(server)
            .execute(&self.pool)
            .await?;
        Ok(actor)
    }
}
impl Drop for Panel {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[sqlx::test]
async fn cloud_resources_apply_scope_before_limit_and_keep_token_intersection(
    pool: PgPool,
) -> Result<()> {
    let panel = Panel::start(pool).await?;
    let first = panel.server("allowed cloud host").await?;
    let second = panel.server("unassigned cloud host").await?;
    let viewer = panel.viewer(first).await?;
    sqlx::query(
        "UPDATE administrator_profiles SET capabilities='[\"cloud:read\"]' WHERE admin_id=$1",
    )
    .bind(viewer)
    .execute(&panel.pool)
    .await?;
    let account = Uuid::new_v4();
    sqlx::query("INSERT INTO alicloud_accounts(id,name,access_key_id,access_key_secret) VALUES($1,'cloud scope fixture','fixture-only','fixture-only')")
        .bind(account).execute(&panel.pool).await?;
    sqlx::query("UPDATE alicloud_accounts SET balance='{}',balance_error='fixture-account-global-error' WHERE id=$1")
        .bind(account).execute(&panel.pool).await?;
    sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id) SELECT gen_random_uuid(),$1,'a hidden','ecs','example-region','i-hidden-'||n FROM generate_series(1,510) n")
        .bind(account).execute(&panel.pool).await?;
    sqlx::query("INSERT INTO operations_cloud_links(resource_id,server_id,updated_at,updated_by) SELECT id,$1,0,1 FROM alicloud_resources WHERE account_id=$2")
        .bind(second).bind(account).execute(&panel.pool).await?;
    let visible = Uuid::new_v4();
    sqlx::query("INSERT INTO alicloud_resources(id,account_id,name,kind,region,cloud_id) VALUES($1,$2,'z allowed','ecs','example-region','i-allowed')")
        .bind(visible).bind(account).execute(&panel.pool).await?;
    sqlx::query("INSERT INTO operations_cloud_links(resource_id,server_id,updated_at,updated_by) VALUES($1,$2,0,1)")
        .bind(visible).bind(first).execute(&panel.pool).await?;
    let cookie = panel.session(viewer, false).await?;
    let response: Value = panel
        .client
        .get(format!("{}/api/operations/cloud", panel.base))
        .header(header::COOKIE, cookie)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let resources = response["resources"]
        .as_array()
        .context("cloud resources")?;
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0]["id"], visible.to_string());
    assert!(resources[0]["balance"].is_null());
    assert!(resources[0]["balance_error"].is_null());
    assert_eq!(resources[0]["balance_scope"], "resource_only");
    let token = format!("apsp_api_{}", random_token());
    sqlx::query("INSERT INTO management_api_tokens(id,admin_id,token_hash,name,capabilities,server_ids,all_servers,expires_at,created_at) VALUES($1,1,$2,'cloud scope','[\"cloud:read\"]',$3,false,$4,0)")
        .bind(Uuid::new_v4()).bind(hash_token(&token)).bind(json!([first])).bind(now_timestamp()+3600).execute(&panel.pool).await?;
    let response: Value = panel
        .client
        .get(format!("{}/api/operations/cloud", panel.base))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let resources = response["resources"]
        .as_array()
        .context("token cloud resources")?;
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0]["id"], visible.to_string());
    assert!(resources[0]["balance"].is_null());
    assert!(resources[0]["balance_error"].is_null());
    let retired = panel.server("retired cloud host").await?;
    sqlx::query("UPDATE servers SET deleted_at=$2 WHERE id=$1")
        .bind(retired)
        .bind(now_timestamp())
        .execute(&panel.pool)
        .await?;
    let response = panel
        .client
        .post(format!(
            "{}/api/operations/cloud/{visible}/link",
            panel.base
        ))
        .header(header::COOKIE, panel.session(1, true).await?)
        .header(header::ORIGIN, &panel.base)
        .json(&json!({"server_id":retired,"purchase_reference":"","notes":""}))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let linked: i64 =
        sqlx::query_scalar("SELECT server_id FROM operations_cloud_links WHERE resource_id=$1")
            .bind(visible)
            .fetch_one(&panel.pool)
            .await?;
    assert_eq!(linked, first);
    Ok(())
}

#[sqlx::test]
async fn scoped_viewer_cannot_read_other_servers_or_mutate(pool: PgPool) -> Result<()> {
    let panel = Panel::start(pool).await?;
    let first = panel.server("allowed").await?;
    let second = panel.server("unassigned").await?;
    let viewer = panel.viewer(first).await?;
    let cookie = panel.session(viewer, false).await?;
    let values: Vec<Value> = panel
        .client
        .get(format!("{}/api/servers", panel.base))
        .header(header::COOKIE, &cookie)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(values.len(), 1);
    assert_eq!(values[0]["id"], first);
    assert_eq!(
        panel
            .client
            .get(format!("{}/api/servers/{second}", panel.base))
            .header(header::COOKIE, &cookie)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        panel
            .client
            .patch(format!("{}/api/servers/{first}", panel.base))
            .header(header::COOKIE, &cookie)
            .json(&json!({"name":"forbidden"}))
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        panel
            .client
            .get(format!("{}/api/settings", panel.base))
            .header(header::COOKIE, &cookie)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let name: String = sqlx::query_scalar("SELECT name FROM servers WHERE id=$1")
        .bind(first)
        .fetch_one(&panel.pool)
        .await?;
    assert_eq!(name, "allowed");
    Ok(())
}

#[sqlx::test]
async fn owner_api_token_retains_capability_and_server_limits(pool: PgPool) -> Result<()> {
    let panel = Panel::start(pool).await?;
    let first = panel.server("allowed").await?;
    let second = panel.server("unassigned").await?;
    let token = format!("apsp_api_{}", random_token());
    sqlx::query("INSERT INTO management_api_tokens(id,admin_id,token_hash,name,capabilities,server_ids,all_servers,expires_at,created_at) VALUES($1,1,$2,'read-only','[\"servers:read\"]',$3,false,$4,0)").bind(Uuid::new_v4()).bind(hash_token(&token)).bind(json!([first])).bind(now_timestamp()+3600).execute(&panel.pool).await?;
    let values: Vec<Value> = panel
        .client
        .get(format!("{}/api/servers", panel.base))
        .bearer_auth(&token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(values.len(), 1);
    assert_eq!(values[0]["id"], first);
    assert_eq!(
        panel
            .client
            .get(format!("{}/api/servers/{second}", panel.base))
            .bearer_auth(&token)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        panel
            .client
            .post(format!("{}/api/servers", panel.base))
            .bearer_auth(&token)
            .json(&json!({"name":"forbidden"}))
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        panel
            .client
            .get(format!("{}/api/settings", panel.base))
            .bearer_auth(&token)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        panel
            .client
            .get(format!(
                "{}/api/control-center/preferences/favorite",
                panel.base
            ))
            .bearer_auth(&token)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        panel
            .client
            .get(format!("{}/api/security/totp", panel.base))
            .bearer_auth(&token)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE management_api_tokens SET revoked_at=$1")
        .bind(now_timestamp())
        .execute(&panel.pool)
        .await?;
    assert_eq!(
        panel
            .client
            .get(format!("{}/api/servers", panel.base))
            .bearer_auth(&token)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[sqlx::test]
async fn scoped_operator_cannot_change_shared_settings_and_denied_reauth_is_audited(
    pool: PgPool,
) -> Result<()> {
    let panel = Panel::start(pool).await?;
    let first = panel.server("allowed").await?;
    let second = panel.server("unassigned").await?;
    sqlx::query("UPDATE servers SET asset_settings=$1 WHERE id=ANY($2)")
        .bind(json!({"price":"1.00","expires_at":1790812800}))
        .bind(vec![first, second])
        .execute(&panel.pool)
        .await?;
    let operator = panel.viewer(first).await?;
    sqlx::query("UPDATE administrator_profiles SET role='operator',capabilities='[\"servers:read\",\"servers:write\",\"diagnostics:read\",\"diagnostics:write\",\"cloud:read\",\"cloud:write\",\"network:read\",\"network:write\"]' WHERE admin_id=$1")
        .bind(operator)
        .execute(&panel.pool)
        .await?;
    let scoped = panel.session(operator, true).await?;
    let owner = panel.session(1, true).await?;
    let send = |method: reqwest::Method, path: &str, cookie: &str, body: Value| {
        panel
            .client
            .request(method, format!("{}{path}", panel.base))
            .header(header::COOKIE, cookie)
            .header(header::ORIGIN, &panel.base)
            .json(&body)
            .send()
    };
    // Tool licences and targets are shared by every administrator.
    let tool = json!({"id":"curl","version":"8.0.0","license":"MIT","source_url":"https://example.com/curl","licensed":true,"platforms":["linux-amd64"]});
    assert_eq!(
        send(
            reqwest::Method::POST,
            "/api/network-workbench/tools",
            &scoped,
            tool.clone()
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    assert!(
        send(
            reqwest::Method::POST,
            "/api/network-workbench/tools",
            &owner,
            tool
        )
        .await?
        .status()
        .is_success()
    );
    // A document without servers stays readable but only a global writer may change it.
    let endpoint = json!({"kind":"endpoint","name":"shared endpoint","server_id":null,
        "listen_address":"127.0.0.1","public_address":null,"port":8080,"protocol":"tcp",
        "owner":"external","notes":""});
    let document = Uuid::new_v4();
    sqlx::query("INSERT INTO network_documents(id,kind,config,created_at,updated_at) VALUES($1,'endpoint',$2,0,0)")
        .bind(document)
        .bind(&endpoint)
        .execute(&panel.pool)
        .await?;
    let path = format!("/api/network-configuration/documents/{document}");
    assert_eq!(
        send(reqwest::Method::GET, &path, &scoped, Value::Null)
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(
            reqwest::Method::PUT,
            &path,
            &scoped,
            json!({"config":endpoint,"revision":1})
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    // Every combined ledger view exposes panel-wide records, while the dedicated
    // server projection remains available and filters by the actor's server scope.
    for path in [
        "/api/costs/subscriptions",
        "/api/costs/summary",
        "/api/costs/payments",
        "/api/costs/calendar?from=2026-10-01&to=2026-10-31",
        "/api/costs/calendar.ics?from=2026-10-01&to=2026-10-31",
    ] {
        assert_eq!(
            send(reqwest::Method::GET, path, &scoped, Value::Null)
                .await?
                .status(),
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    let server_expenses: Value = send(
        reqwest::Method::GET,
        "/api/costs/servers",
        &scoped,
        Value::Null,
    )
    .await?
    .error_for_status()?
    .json()
    .await?;
    assert_eq!(server_expenses["entries"].as_array().unwrap().len(), 1);
    assert_eq!(server_expenses["entries"][0]["id"], first);
    // The generic runtime barrier is owner-only.
    let barrier = format!("/api/servers/{first}/runtime-control/barrier");
    let input = json!({"module":"example","minimum_revision":1});
    assert_eq!(
        send(reqwest::Method::POST, &barrier, &scoped, input.clone())
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_ne!(
        send(reqwest::Method::POST, &barrier, &owner, input)
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    // A high-risk request refused for a missing re-verification leaves an audit row.
    let stale = panel.session(1, false).await?;
    let target = format!("/api/network-workbench/targets/{}", Uuid::new_v4());
    assert_eq!(
        send(reqwest::Method::DELETE, &target, &stale, Value::Null)
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let denied: i64 = sqlx::query_scalar("SELECT count(*) FROM management_audit WHERE object_path=$1 AND result->>'phase'='reauth-denied'")
        .bind(&target)
        .fetch_one(&panel.pool)
        .await?;
    assert_eq!(denied, 1);
    Ok(())
}

#[sqlx::test]
async fn proof_is_session_bound_and_preference_conflicts_preserve_values(
    pool: PgPool,
) -> Result<()> {
    let panel = Panel::start(pool).await?;
    let stale = panel.session(1, false).await?;
    let recent = panel.session(1, true).await?;
    let body = json!({"name":"limited","capabilities":["servers:read"],"server_ids":[],"all_servers":false,"expires_at":now_timestamp()+3600});
    assert_eq!(
        panel
            .client
            .post(format!("{}/api/control-center/tokens", panel.base))
            .header(header::COOKIE, &stale)
            .json(&body)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = panel
        .client
        .post(format!("{}/api/control-center/tokens", panel.base))
        .header(header::COOKIE, &recent)
        .json(&body)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value = response.json().await?;
    let issued = value["token"].as_str().context("issued token")?.to_owned();
    assert!(issued.starts_with("apsp_api_"));
    let url = format!("{}/api/control-center/preferences/view:servers", panel.base);
    assert_eq!(
        panel
            .client
            .put(&url)
            .header(header::COOKIE, &recent)
            .json(&json!({"value":{"sort":"name"},"expected_revision":0}))
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        panel
            .client
            .put(&url)
            .header(header::COOKIE, &recent)
            .json(&json!({"value":{"sort":"cpu"},"expected_revision":0}))
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let value: Value = panel
        .client
        .get(&url)
        .header(header::COOKIE, &recent)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(value["value"]["sort"], "name");
    let audit: Vec<Value> = panel
        .client
        .get(format!("{}/api/control-center/audit", panel.base))
        .header(header::COOKIE, &recent)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(!serde_json::to_string(&audit)?.contains(&issued));
    Ok(())
}

#[sqlx::test]
async fn authorization_changes_revoke_existing_credentials_and_protect_the_last_owner(
    pool: PgPool,
) -> Result<()> {
    let panel = Panel::start(pool).await?;
    let first = panel.server("first").await?;
    let second = panel.server("second").await?;
    let viewer = panel.viewer(first).await?;
    let owner = panel.session(1, true).await?;
    let session = panel.session(viewer, false).await?;
    let token = format!("apsp_api_{}", random_token());
    sqlx::query("INSERT INTO management_api_tokens(id,admin_id,token_hash,name,capabilities,server_ids,all_servers,expires_at,created_at) VALUES($1,$2,$3,'viewer','[\"servers:read\"]',$4,false,$5,0)")
        .bind(Uuid::new_v4()).bind(viewer).bind(hash_token(&token)).bind(json!([first])).bind(now_timestamp()+3600).execute(&panel.pool).await?;
    let update = json!({"login_name":"viewer","display_name":"只读","role":"viewer","enabled":true,"all_servers":false,"capabilities":["servers:read"],"server_ids":[second],"password":null,"expected_revision":1});
    let url = format!("{}/api/control-center/administrators/{viewer}", panel.base);
    assert_eq!(
        panel
            .client
            .put(&url)
            .header(header::COOKIE, &owner)
            .json(&update)
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        panel
            .client
            .get(format!("{}/api/servers", panel.base))
            .header(header::COOKIE, &session)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        panel
            .client
            .get(format!("{}/api/servers", panel.base))
            .bearer_auth(&token)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        panel
            .client
            .put(&url)
            .header(header::COOKIE, &owner)
            .json(&update)
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let last_owner = json!({"login_name":"admin","display_name":"所有者","role":"viewer","enabled":true,"all_servers":false,"capabilities":["servers:read"],"server_ids":[first],"password":null,"expected_revision":1});
    assert_eq!(
        panel
            .client
            .put(format!(
                "{}/api/control-center/administrators/1",
                panel.base
            ))
            .header(header::COOKIE, &owner)
            .json(&last_owner)
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let role: String =
        sqlx::query_scalar("SELECT role FROM administrator_profiles WHERE admin_id=1")
            .fetch_one(&panel.pool)
            .await?;
    assert_eq!(role, "owner");
    let response = panel
        .client
        .post(format!("{}/api/login", panel.base))
        .json(&json!({"login_name":"viewer","password":"control-test-password"}))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let authenticated: Value = response.json().await?;
    assert_eq!(authenticated["id"], viewer);
    assert_eq!(
        panel
            .client
            .post(format!("{}/api/login", panel.base))
            .json(&json!({"login_name":"viewer","password":"wrong-sensitive-password"}))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let audits: Vec<Value> = sqlx::query_scalar(
        "SELECT request_diff || result FROM management_audit WHERE action='login'",
    )
    .fetch_all(&panel.pool)
    .await?;
    assert!(audits.iter().any(|value| value["phase"] == "authenticated"));
    assert!(
        audits
            .iter()
            .any(|value| value["phase"] == "authentication-denied")
    );
    let serialized = serde_json::to_string(&audits)?;
    assert!(!serialized.contains("control-test-password"));
    assert!(!serialized.contains("wrong-sensitive-password"));
    Ok(())
}

#[sqlx::test]
async fn background_heartbeats_are_stored_by_the_platform_and_shown_in_the_health_view(
    pool: PgPool,
) -> Result<()> {
    let panel = Panel::start(pool.clone()).await?;
    // A fixture loop whose first pass succeeds and whose later passes fail; the pass
    // count lives in the loop's context.
    let task = tokio::spawn(periodic(
        "fixture-loop",
        pool.clone(),
        Cadence::every(Duration::from_millis(20)),
        0_u64,
        |passes| {
            Box::pin(async move {
                *passes += 1;
                if *passes == 1 {
                    Ok(json!({"source":"fixture","pass":1}))
                } else {
                    Err(Failure::new(
                        "fixture failure",
                        json!({"source":"fixture","pass":*passes,"error":"e".repeat(600),"token":"TEST_ONLY"}),
                    ))
                }
            })
        },
    ));
    let stored = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let row: Option<(String, i64, Option<i64>, Value)> = sqlx::query_as("SELECT status,observed_at,last_success_at,details FROM system_worker_heartbeats WHERE name='fixture-loop'")
                .fetch_optional(&pool)
                .await?;
            if let Some(row) = row
                && row.3["pass"].as_u64() >= Some(3)
            {
                return Ok::<_, sqlx::Error>(row);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("the fixture loop records its heartbeat")??;
    task.abort();
    let _ = task.await;
    let (status, observed_at, last_success_at, details) = stored;
    assert_eq!(status, "failed");
    assert!(
        last_success_at.is_some_and(|success| success <= observed_at),
        "a failed pass keeps the time of the last success"
    );
    assert_eq!(
        details["error"].as_str().map(|error| error.chars().count()),
        Some(512),
        "stored details are bounded"
    );
    let cookie = panel.session(1, false).await?;
    let health: Value = panel
        .client
        .get(format!("{}/api/control-center/health", panel.base))
        .header(header::COOKIE, cookie)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let workers = health["workers"].as_array().context("workers")?;
    let fixture = workers
        .iter()
        .find(|worker| worker["name"] == "fixture-loop")
        .context("the fixture heartbeat is listed")?;
    assert_eq!(fixture["status"], "failed");
    assert_eq!(fixture["details"]["source"], "fixture");
    assert_eq!(
        fixture["details"]["token"], "[已脱敏]",
        "the view redacts details as the audit does"
    );
    // No background loop runs in the test panel, so every expected name, including the
    // tasks that report a heartbeat for the first time, is listed as unknown.
    for name in [
        "maintenance",
        "expenses",
        "exchange-rates",
        "operations-retention",
        "subscription-sources",
        "member-messages",
        "subscription-hosts",
        "china-reachability",
    ] {
        let worker = workers
            .iter()
            .find(|worker| worker["name"] == name)
            .with_context(|| format!("{name} is expected"))?;
        assert_eq!(worker["status"], "unknown", "{name}");
    }
    Ok(())
}

#[sqlx::test]
async fn the_health_view_joins_the_panel_with_each_owners_backlog_and_backups(
    pool: PgPool,
) -> Result<()> {
    let panel = Panel::start(pool.clone()).await?;
    let servers = [
        panel.server("health first").await?,
        panel.server("health second").await?,
    ];
    let now = now_timestamp();
    // Operations: four unfinished commands, three unreconciled device operations and five
    // unfinished jobs, each next to finished ones that are not counted.
    for state in [
        "queued",
        "claimed",
        "running",
        "cancel_requested",
        "succeeded",
    ] {
        sqlx::query("INSERT INTO remote_commands(id,server_id,requested_at,spec,state) VALUES($1,$2,0,'{}',$3)")
            .bind(Uuid::new_v4()).bind(servers[0]).bind(state).execute(&pool).await?;
    }
    for (status, reconciled) in [
        ("queued", None),
        ("dispatched", None),
        ("unknown", None),
        ("unknown", Some(now)),
        ("succeeded", None),
    ] {
        sqlx::query("INSERT INTO fleet_operations(id,server_id,operation,policy,requested_at,expires_at,status,reconciled_at) VALUES($1,$2,'{}','{}',0,0,$3,$4)")
            .bind(Uuid::new_v4()).bind(servers[0]).bind(status).bind(reconciled).execute(&pool).await?;
    }
    for status in [
        "queued",
        "running",
        "paused",
        "cancel_requested",
        "uncertain",
        "succeeded",
    ] {
        sqlx::query("INSERT INTO operations_jobs(id,name,requested_by,spec,targets,status,created_at,updated_at,expires_at,preview_digest) VALUES($1,'health fixture',1,'{}',$2,$3,0,0,0,'fixture')")
            .bind(Uuid::new_v4()).bind(vec![servers[0]]).bind(status).execute(&pool).await?;
    }
    // Observability: two active diagnostic jobs (one per server) and one waiting alert.
    for (server, status) in [
        (servers[0], "queued"),
        (servers[1], "running"),
        (servers[0], "succeeded"),
    ] {
        sqlx::query("INSERT INTO diagnostic_jobs(id,server_id,job,status,created_at,updated_at,expires_at) VALUES($1,$2,'{}',$3,0,0,0)")
            .bind(Uuid::new_v4()).bind(server).bind(status).execute(&pool).await?;
    }
    let event: i64 = sqlx::query_scalar("INSERT INTO server_alert_events(server_id,server_name,opened_at) VALUES($1,'fixture',1) RETURNING id")
        .bind(servers[0]).fetch_one(&pool).await?;
    for (channel, status) in [("telegram", "pending"), ("webhook", "sent")] {
        sqlx::query("INSERT INTO notification_outbox(event_id,kind,channel,message,status,next_attempt_at) VALUES($1,'offline',$2,'fixture',$3,0)")
            .bind(event).bind(channel).bind(status).execute(&pool).await?;
    }
    // Backups: the newest kept record with its latest drill, schedules by last change.
    let (older, newer) = (Uuid::new_v4(), Uuid::new_v4());
    for (id, name, updated, error) in [
        (older, "older plan", 10_i64, None),
        (newer, "newer plan", 20, Some("fixture failure")),
    ] {
        sqlx::query("INSERT INTO operations_backup_schedules(id,name,requested_by,interval_secs,next_run_at,recipient,retention_count,retention_days,last_error,created_at,updated_at) VALUES($1,$2,1,86400,$3,'fixture',7,30,$4,0,$3)")
            .bind(id).bind(name).bind(updated).bind(error).execute(&pool).await?;
    }
    let (kept, retired) = (Uuid::new_v4(), Uuid::new_v4());
    for (id, name, created, retired_at) in [
        (kept, "kept backup", 100_i64, None),
        (retired, "retired backup", 200, Some(300_i64)),
    ] {
        sqlx::query("INSERT INTO operations_backup_records(id,name,manifest,manifest_sha256,location,encrypted,verification,created_at,imported_at,imported_by,retired_at) VALUES($1,$2,'{}','fixture','fixture',true,'integrity_verified',$3,$3,1,$4)")
            .bind(id).bind(name).bind(created).bind(retired_at).execute(&pool).await?;
    }
    for (recorded, passed, action) in [(110_i64, false, "restore"), (120, true, "verify")] {
        sqlx::query("INSERT INTO operations_restore_drills(id,backup_id,report,report_sha256,passed,recorded_at,recorded_by) VALUES($1,$2,$3,'fixture',$4,$5,1)")
            .bind(Uuid::new_v4()).bind(kept).bind(json!({"action":action})).bind(passed).bind(recorded).execute(&pool).await?;
    }
    // Governance: an independent observer's report.
    sqlx::query("INSERT INTO system_observer_reports(observer_name,target_origin,observed_at,received_at,available,elapsed_ms,evidence) VALUES('fixture observer',$1,$2,$2,true,12,'{\"status\":200}')")
        .bind(&panel.base).bind(now).execute(&pool).await?;

    let url = format!("{}/api/control-center/health", panel.base);
    let cookie = panel.session(1, false).await?;
    let health: Value = panel
        .client
        .get(&url)
        .header(header::COOKIE, cookie)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let keys: Vec<&str> = health
        .as_object()
        .context("the view is an object")?
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "backlog",
            "backup",
            "database",
            "disk",
            "independent_observers",
            "sampled_at",
            "update",
            "uptime_seconds",
            "version",
            "workers"
        ]
    );
    assert_eq!(health["version"], env!("CARGO_PKG_VERSION"));
    assert!(health["sampled_at"].as_i64() >= Some(now));
    assert!(health["uptime_seconds"].is_i64());
    assert_eq!(
        health["backlog"],
        json!({"commands":4,"diagnostics":2,"notifications":1,"fleet":3,"operations":5})
    );
    assert_eq!(
        health["backup"],
        json!({
            "status": "recorded",
            "latest": {"id":kept,"name":"kept backup","created_at":100,"encrypted":true,"verification":"integrity_verified","last_drill":{"passed":true,"recorded_at":120,"action":"verify"}},
            "schedules": [
                {"id":newer,"name":"newer plan","paused":false,"next_run_at":20,"last_started_at":null,"last_finished_at":null,"last_error":"fixture failure"},
                {"id":older,"name":"older plan","paused":false,"next_run_at":10,"last_started_at":null,"last_finished_at":null,"last_error":null},
            ],
            "source": "local-backup-ledger",
        })
    );
    assert_eq!(health["database"]["available"], true);
    for key in ["elapsed_ms", "bytes", "pool_size", "idle"] {
        assert!(health["database"][key].is_u64(), "database {key}");
    }
    assert!(health["disk"]["status"].is_string());
    assert_eq!(
        health["independent_observers"],
        json!([{"observer_name":"fixture observer","target_origin":panel.base,"observed_at":now,"received_at":now,"available":true,"elapsed_ms":12,"evidence":{"status":200}}])
    );
    assert_eq!(health["update"]["remote_checked"], false);
    assert!(health["workers"].is_array());
    // Only an owner session sees the view.
    let viewer = panel.viewer(servers[0]).await?;
    let viewer = panel.session(viewer, false).await?;
    assert_eq!(
        panel
            .client
            .get(&url)
            .header(header::COOKIE, viewer)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    Ok(())
}

#[sqlx::test]
async fn each_owner_purges_its_own_records_past_the_retention_policy(pool: PgPool) -> Result<()> {
    let panel = Panel::start(pool.clone()).await?;
    let server = panel.server("retention fixture").await?;
    // Distinct periods, so a record purged under another kind's period shows.
    sqlx::query("UPDATE record_retention_policy SET days=CASE kind WHEN 'audit' THEN 400 WHEN 'ip' THEN 20 WHEN 'terminal' THEN 5 WHEN 'logs' THEN 10 ELSE days END")
        .execute(&pool).await?;
    let now = now_timestamp();
    let past = |days: i64| now - (days + 1) * 86_400;
    let within = |days: i64| now - (days - 1) * 86_400;

    // Governance (audit): the request audit, observer reports and API tokens.
    for (path, at) in [
        ("/fixture/past", past(400)),
        ("/fixture/within", within(400)),
    ] {
        sqlx::query("INSERT INTO management_audit(action,object_path,request_diff,result,occurred_at) VALUES('GET',$1,'{}','{}',$2)")
            .bind(path).bind(at).execute(&pool).await?;
        sqlx::query("INSERT INTO system_observer_reports(observer_name,target_origin,observed_at,received_at,evidence) VALUES($1,'fixture',$2,$2,'{}')")
            .bind(path).bind(at).execute(&pool).await?;
    }
    for (name, expires, revoked) in [
        ("expired", past(400), None),
        ("revoked within", past(400), Some(within(400))),
        ("valid", within(400), None),
    ] {
        sqlx::query("INSERT INTO management_api_tokens(id,admin_id,token_hash,name,capabilities,server_ids,expires_at,revoked_at,created_at) VALUES($1,1,$2,$3,'[]','[]',$4,$5,0)")
            .bind(Uuid::new_v4()).bind(format!("fixture {name}")).bind(name).bind(expires).bind(revoked).execute(&pool).await?;
    }
    // Observability: IP quality results (ip) and server events (logs).
    for (ip, checked, dataset) in [
        ("192.0.2.1", past(20), Some(past(20))),
        ("192.0.2.2", past(20), Some(within(20))),
        ("192.0.2.3", within(20), None),
    ] {
        sqlx::query(
            "INSERT INTO server_ip_quality(server_id,ip,payload,checked_at) VALUES($1,$2,'{}',$3)",
        )
        .bind(server)
        .bind(ip)
        .bind(checked)
        .execute(&pool)
        .await?;
        if let Some(at) = dataset {
            sqlx::query("INSERT INTO server_ip_quality_datasets(server_id,ip,provider,database,checked_at,last_attempt) VALUES($1,$2,'check-place','maxmind',$3,'{}')")
                .bind(server).bind(ip).bind(at).execute(&pool).await?;
        }
    }
    for (kind, at) in [("past", past(10)), ("within", within(10))] {
        sqlx::query("INSERT INTO fleet_events(id,server_id,kind,source,occurred_at,detail) VALUES($1,$2,$3,'fixture',$4,'{}')")
            .bind(Uuid::new_v4()).bind(server).bind(kind).bind(at).execute(&pool).await?;
    }
    // Operations: closed terminal sessions (terminal), and the contents of configuration
    // history, finished device operations and command output (logs).
    for (status, expires) in [
        ("closed", past(5)),
        ("running", past(5)),
        ("failed", within(5)),
    ] {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO fleet_terminal_sessions(id,server_id,admin_id,account,admin_session_hash,policy,columns,rows,created_at,expires_at,last_input_at,status) VALUES($1,$2,1,'root',$3,'{}',80,24,0,$4,0,$5)")
            .bind(id).bind(server).bind(format!("fixture {status}")).bind(expires).bind(status).execute(&pool).await?;
        sqlx::query(
            "INSERT INTO fleet_terminal_inputs(session_id,sequence,data) VALUES($1,1,'input')",
        )
        .bind(id)
        .execute(&pool)
        .await?;
        sqlx::query("INSERT INTO fleet_terminal_outputs(session_id,sequence,data,digest,created_at) VALUES($1,1,'output','fixture',0)")
            .bind(id).execute(&pool).await?;
    }
    let write = json!({"kind":"write","content":"secret"});
    let operations = [
        (
            "succeeded",
            None,
            past(10),
            write.clone(),
            Some(json!({"ok":true})),
        ),
        ("dispatched", None, past(10), write.clone(), None),
        (
            "unknown",
            Some(now),
            past(10),
            json!({"kind":"service"}),
            Some(json!({"ok":false})),
        ),
        (
            "succeeded",
            None,
            within(10),
            write.clone(),
            Some(json!({"ok":true})),
        ),
    ]
    .map(|(status, reconciled, requested, operation, result)| {
        (
            Uuid::new_v4(),
            status,
            reconciled,
            requested,
            operation,
            result,
        )
    });
    for (id, status, reconciled, requested, operation, result) in &operations {
        sqlx::query("INSERT INTO fleet_operations(id,server_id,operation,policy,requested_at,expires_at,status,result,reconciled_at) VALUES($1,$2,$3,'{}',$4,$4,$5,$6,$7)")
            .bind(id).bind(server).bind(operation).bind(requested).bind(status).bind(result).bind(reconciled).execute(&pool).await?;
    }
    for (operation, path, created) in [
        (operations[0].0, "/etc/past.conf", past(10)),
        (operations[3].0, "/etc/within.conf", within(10)),
    ] {
        sqlx::query("INSERT INTO fleet_config_history(id,server_id,path,operation_id,previous_sha256,sha256,content,created_at) VALUES($1,$2,$3,$4,'fixture','fixture','configuration',$5)")
            .bind(Uuid::new_v4()).bind(server).bind(path).bind(operation).bind(created).execute(&pool).await?;
    }
    let output = json!({"stdout":"out","stderr":"err","exit_code":0});
    for (state, finished) in [
        ("succeeded", past(10)),
        ("running", past(10)),
        ("failed", within(10)),
    ] {
        sqlx::query("INSERT INTO remote_commands(id,server_id,requested_at,spec,state,result,finished_at) VALUES($1,$2,0,'{}',$3,$4,$5)")
            .bind(Uuid::new_v4()).bind(server).bind(state).bind(&output).bind(finished).execute(&pool).await?;
    }

    apsp_governance::control_center::retention::purge(&pool, now).await?;
    apsp_observability::retention::purge(&pool, now).await?;
    apsp_operations::retention::purge(&pool, now).await?;

    let audit: Vec<String> = sqlx::query_scalar("SELECT object_path FROM management_audit WHERE object_path LIKE '/fixture/%' ORDER BY object_path")
        .fetch_all(&pool).await?;
    assert_eq!(audit, ["/fixture/within"]);
    let observers: Vec<String> = sqlx::query_scalar(
        "SELECT observer_name FROM system_observer_reports ORDER BY observer_name",
    )
    .fetch_all(&pool)
    .await?;
    assert_eq!(observers, ["/fixture/within"]);
    let tokens: Vec<String> =
        sqlx::query_scalar("SELECT name FROM management_api_tokens ORDER BY name")
            .fetch_all(&pool)
            .await?;
    assert_eq!(tokens, ["revoked within", "valid"]);
    let quality: Vec<(String, i64)> = sqlx::query_as("SELECT q.ip,(SELECT count(*) FROM server_ip_quality_datasets d WHERE d.server_id=q.server_id AND d.ip=q.ip) FROM server_ip_quality q WHERE q.server_id=$1 ORDER BY q.ip")
        .bind(server).fetch_all(&pool).await?;
    assert_eq!(
        quality,
        [("192.0.2.2".to_owned(), 1), ("192.0.2.3".to_owned(), 0)],
        "a result goes once it and all its datasets are past the period"
    );
    let events: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM fleet_events WHERE server_id=$1 ORDER BY kind")
            .bind(server)
            .fetch_all(&pool)
            .await?;
    assert_eq!(events, ["within"]);
    let terminals: Vec<(String, i64, i64)> = sqlx::query_as("SELECT s.status,(SELECT count(*) FROM fleet_terminal_inputs i WHERE i.session_id=s.id),(SELECT count(*) FROM fleet_terminal_outputs o WHERE o.session_id=s.id) FROM fleet_terminal_sessions s WHERE s.server_id=$1 ORDER BY s.status")
        .bind(server).fetch_all(&pool).await?;
    assert_eq!(
        terminals,
        [("failed".to_owned(), 1, 1), ("running".to_owned(), 1, 1)],
        "only closed sessions past the period go, with their input and output"
    );
    let stored: Vec<(Value, Option<Value>)> = sqlx::query_as("SELECT operation,result FROM fleet_operations WHERE id=ANY($1) ORDER BY array_position($1,id)")
        .bind(operations.iter().map(|operation| operation.0).collect::<Vec<_>>()).fetch_all(&pool).await?;
    assert_eq!(
        stored,
        [
            (json!({"kind":"write","content":""}), None),
            (write.clone(), None),
            (json!({"kind":"service"}), None),
            (write, Some(json!({"ok":true}))),
        ],
        "finished or reconciled operations past the period keep their rows without contents"
    );
    let history: Vec<(String, String)> = sqlx::query_as(
        "SELECT path,content FROM fleet_config_history WHERE server_id=$1 ORDER BY path",
    )
    .bind(server)
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        history,
        [
            ("/etc/past.conf".to_owned(), String::new()),
            ("/etc/within.conf".to_owned(), "configuration".to_owned()),
        ]
    );
    let commands: Vec<(String, Value)> = sqlx::query_as(
        "SELECT state,result FROM remote_commands WHERE server_id=$1 ORDER BY state",
    )
    .bind(server)
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        commands,
        [
            ("failed".to_owned(), output.clone()),
            ("running".to_owned(), output),
            (
                "succeeded".to_owned(),
                json!({"stdout":"[按保留策略清理]","stderr":"[按保留策略清理]","exit_code":0})
            ),
        ]
    );
    Ok(())
}

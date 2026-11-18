#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result};
use apsp_governance::auth::{hash_token, random_token};
use apsp_protocol::now_timestamp;
use business_support::TestPanel;
use reqwest::{Method, StatusCode, header};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

fn domain(server: i64) -> Value {
    json!({"kind":"domain","name":"network.example.com","server_ids":[server],"ddns_rule_ids":[],"applications":[],"maintainer":"TEST_ONLY","notes":""})
}
fn forwarding(server: i64, address: &str) -> Value {
    json!({"kind":"forwarding","name":"TEST_ONLY forwarding","server_id":server,"listen_address":address,"listen_port":18080,"target_address":"127.0.0.1","target_port":8080,"protocol":"tcp","owner":"apsp","enabled":true,"dependency_ids":[]})
}

#[sqlx::test]
async fn revisions_and_wildcard_listeners_preserve_existing_configuration(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "TEST_ONLY network").await?;
    let created: Value = panel
        .admin(
            Method::POST,
            "/api/network-configuration/documents",
            &cookie,
            Some(json!({"config":forwarding(server,"0.0.0.0")})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = created["id"].as_str().context("document ID")?;
    let conflict = panel
        .admin(
            Method::POST,
            "/api/network-configuration/documents",
            &cookie,
            Some(json!({"config":forwarding(server,"127.0.0.1")})),
        )
        .await?;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let mut replacement = created["config"].clone();
    replacement["target_port"] = 8081.into();
    let stale = panel
        .admin(
            Method::PUT,
            &format!("/api/network-configuration/documents/{id}"),
            &cookie,
            Some(json!({"config":replacement,"revision":0})),
        )
        .await?;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let after: Value = panel
        .admin(
            Method::GET,
            &format!("/api/network-configuration/documents/{id}"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(after["config"]["target_port"], 8080);
    assert_eq!(after["revision"], 1);
    let operations: i64 = sqlx::query_scalar("SELECT count(*) FROM fleet_operations")
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        operations, 0,
        "saving or previewing a configuration must not dispatch work"
    );
    Ok(())
}

#[sqlx::test]
async fn old_server_history_is_hidden_after_scope_changes(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let owner = panel.admin_cookie().await?;
    let first = panel.create_server(&owner, "TEST_ONLY old scope").await?;
    let second = panel
        .create_server(&owner, "TEST_ONLY visible scope")
        .await?;
    let created: Value = panel
        .admin(
            Method::POST,
            "/api/network-configuration/documents",
            &owner,
            Some(json!({"config":domain(first)})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = created["id"].as_str().context("document ID")?;
    panel
        .admin(
            Method::PUT,
            &format!("/api/network-configuration/documents/{id}"),
            &owner,
            Some(json!({"config":domain(second),"revision":1})),
        )
        .await?
        .error_for_status()?;
    let actor:i64=sqlx::query_scalar("INSERT INTO admins(password_hash) SELECT password_hash FROM admins WHERE id=1 RETURNING id").fetch_one(&pool).await?;
    sqlx::query("INSERT INTO administrator_profiles(admin_id,login_name,display_name,role,all_servers,capabilities,created_at,updated_at) VALUES($1,'network-viewer','网络只读','viewer',false,'[\"network:read\"]',0,0)").bind(actor).execute(&pool).await?;
    sqlx::query("INSERT INTO administrator_server_grants(admin_id,server_id) VALUES($1,$2)")
        .bind(actor)
        .bind(second)
        .execute(&pool)
        .await?;
    let token = random_token();
    sqlx::query("INSERT INTO sessions(token_hash,admin_id,expires_at) VALUES($1,$2,$3)")
        .bind(hash_token(&token))
        .bind(actor)
        .bind(now_timestamp() + 3600)
        .execute(&pool)
        .await?;
    let cookie = format!("apsp_session={token}");
    let history: Vec<Value> = panel
        .admin(
            Method::GET,
            &format!("/api/network-configuration/documents/{id}/history"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["revision"], 2);
    assert_eq!(history[0]["config"]["server_ids"], json!([second]));
    Ok(())
}

#[sqlx::test]
async fn api_tokens_cannot_leave_server_operations_without_token_identity(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel
        .create_server(&cookie, "TEST_ONLY token scope")
        .await?;
    let token = format!("apsp_api_{}", random_token());
    sqlx::query("INSERT INTO management_api_tokens(id,admin_id,token_hash,name,capabilities,server_ids,all_servers,expires_at,created_at) VALUES($1,1,$2,'TEST_ONLY network token','[\"network:read\"]',$3,false,$4,0)").bind(Uuid::new_v4()).bind(hash_token(&token)).bind(json!([server])).bind(now_timestamp()+3600).execute(&pool).await?;
    let response = panel
        .client
        .post(format!(
            "{}/api/network-configuration/servers/{server}/inventory",
            panel.base
        ))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fleet_operations")
            .fetch_one(&pool)
            .await?,
        0
    );
    Ok(())
}

#[sqlx::test]
async fn acme_plan_pins_builtin_lego_and_surfaces_actionable_remediation_for_legacy_versions(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    business_support::prove(&pool, &cookie).await?;
    let server = panel.create_server(&cookie, "TEST_ONLY acme panel").await?;
    let credential = apsp_governance::control_center::credentials::store_generated(
        &pool,
        "TEST_ONLY Cloudflare ACME",
        "dns",
        json!({"provider": "cloudflare", "api_token": "TEST_ONLY_CF_DNS_TOKEN"}),
    )
    .await?;
    let ddns_rule = Uuid::new_v4();
    sqlx::query("INSERT INTO ddns_rules(id,server_id,config,api_token) VALUES($1,$2,$3,'')")
        .bind(ddns_rule)
        .bind(server)
        .bind(json!({
            "name": "TEST_ONLY ddns",
            "provider": "cloudflare",
            "server_id": server,
            "zone_id": "00000000000000000000000000000001",
            "record_name": "network.example.com",
            "record_type": "A",
            "ttl": 300,
            "proxied": false,
            "interval_secs": 300,
            "enabled": true,
            "adopt_existing": false,
            "address_source": "agent"
        }))
        .execute(&pool)
        .await?;

    let created_domain: Value = panel
        .admin(
            Method::POST,
            "/api/network-configuration/documents",
            &cookie,
            Some(json!({"config": domain(server)})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let domain_id = Uuid::parse_str(created_domain["id"].as_str().context("domain ID")?)?;

    let created_cert: Value = panel
        .admin(
            Method::POST,
            "/api/network-configuration/documents",
            &cookie,
            Some(json!({
                "config": {
                    "kind": "certificate",
                    "name": "TEST_ONLY certificate",
                    "domain_ids": [domain_id],
                    "maintainer": "TEST_ONLY",
                    "issuer": "Let's Encrypt",
                    "targets": [],
                    "renewal": {
                        "mode": "dns01",
                        "responsibility": "panel",
                        "ddns_rule_id": ddns_rule
                    }
                }
            })),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let cert_id = Uuid::parse_str(created_cert["id"].as_str().context("certificate ID")?)?;

    let initial: Value = panel
        .admin(
            Method::GET,
            &format!("/api/network-configuration/certificates/{cert_id}/acme-plan"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(initial["tool"], "lego");
    assert_eq!(initial["builtin_tool_version"], "5.6.0");
    assert_eq!(initial["default_tool_version"], "5.6.0");
    assert_eq!(initial["tool_path"], "/usr/local/libexec/apsp/lego");
    assert_eq!(initial["tool_supply"], "panel_builtin_verified_binary");
    assert!(initial["tool_version_mismatch"].is_null());

    let make_plan = |tool_version: &str, license_accepted: bool| {
        json!({
            "tool_version": tool_version,
            "credential_id": credential,
            "email": "ops@example.com",
            "staging": true,
            "terms_accepted": true,
            "license_accepted": license_accepted,
            "automatic_renewal": false,
            "renew_before_days": 30
        })
    };

    assert_eq!(
        panel
            .admin(
                Method::PUT,
                &format!("/api/network-configuration/certificates/{cert_id}/acme-plan"),
                &cookie,
                Some(json!({"config": make_plan("latest", true), "revision": null})),
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        panel
            .admin(
                Method::PUT,
                &format!("/api/network-configuration/certificates/{cert_id}/acme-plan"),
                &cookie,
                Some(json!({"config": make_plan("5.6.0", false), "revision": null})),
            )
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );

    let mismatch_save = panel
        .admin(
            Method::PUT,
            &format!("/api/network-configuration/certificates/{cert_id}/acme-plan"),
            &cookie,
            Some(json!({"config": make_plan("4.20.0", true), "revision": null})),
        )
        .await?;
    assert_eq!(mismatch_save.status(), StatusCode::CONFLICT);
    let mismatch_body: Value = mismatch_save.json().await?;
    assert!(
        mismatch_body["error"]
            .as_str()
            .is_some_and(|msg| msg.contains("4.20.0") && msg.contains("5.6.0"))
    );

    panel
        .admin(
            Method::PUT,
            &format!("/api/network-configuration/certificates/{cert_id}/acme-plan"),
            &cookie,
            Some(json!({"config": make_plan("5.6.0", true), "revision": null})),
        )
        .await?
        .error_for_status()?;

    // Simulate a pre-upgrade stored plan pinned to historical 4.20.0 with automatic_renewal=true.
    let mut legacy = make_plan("4.20.0", true);
    legacy["automatic_renewal"] = json!(true);
    sqlx::query("UPDATE network_acme_plans SET config=$2,next_run_at=0 WHERE certificate_id=$1")
        .bind(cert_id)
        .bind(&legacy)
        .execute(&pool)
        .await?;

    let legacy_view: Value = panel
        .admin(
            Method::GET,
            &format!("/api/network-configuration/certificates/{cert_id}/acme-plan"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        legacy_view["tool_version_mismatch"]["code"],
        "issuer_tool_version_mismatch"
    );
    assert_eq!(
        legacy_view["tool_version_mismatch"]["configured_tool_version"],
        "4.20.0"
    );
    assert_eq!(
        legacy_view["tool_version_mismatch"]["builtin_tool_version"],
        "5.6.0"
    );

    let manual_issue = panel
        .admin(
            Method::POST,
            &format!("/api/network-configuration/certificates/{cert_id}/issue"),
            &cookie,
            Some(json!({"confirmed": true, "remote_reconciled": false})),
        )
        .await?;
    assert_eq!(manual_issue.status(), StatusCode::CONFLICT);

    let worker = tokio::spawn(apsp_naming::run_certificates(panel.state.clone()));
    let mut jobs = Vec::<Value>::new();
    for _ in 0..40 {
        jobs = panel
            .admin(
                Method::GET,
                &format!("/api/network-configuration/certificates/{cert_id}/issuance"),
                &cookie,
                None,
            )
            .await?
            .error_for_status()?
            .json()
            .await?;
        if jobs.first().is_some_and(|job| job["status"] == "failed") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    worker.abort();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["status"], "failed");
    assert_eq!(
        jobs[0]["result"]["error_code"],
        "issuer_tool_version_mismatch"
    );
    assert_eq!(jobs[0]["result"]["requested_tool_version"], "4.20.0");
    assert_eq!(jobs[0]["result"]["builtin_tool_version"], "5.6.0");
    assert_eq!(jobs[0]["result"]["issued"], false);
    assert_eq!(jobs[0]["result"]["cleanup_required"], false);
    Ok(())
}

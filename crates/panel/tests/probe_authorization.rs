#![forbid(unsafe_code)]
mod business_support;
#[path = "probe_support.rs"]
mod probe_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_protocol::now_timestamp;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

fn legacy() -> Value {
    json!({"id":Uuid::nil(),"name":"自有线路","kind":"tcp","target":"probe.example.com","port":443,"interval_secs":30,"carrier":"telecom","enabled":true})
}

#[sqlx::test]
async fn missing_authorization_cannot_enable_and_legacy_executor_never_receives_new_fields(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "authorization").await?;
    let path = format!("/api/servers/{server}/probes");
    let mut forged = legacy();
    forged["execution_authorized"] = json!(true);
    assert_eq!(
        panel
            .admin(Method::POST, &path, &cookie, Some(forged))
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let spec: Value = panel
        .admin(
            Method::POST,
            &path,
            &cookie,
            Some(probe_support::authorized(legacy())),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(spec["execution_authorized"], true);
    let old: Vec<Value> = panel
        .client
        .get(format!("{}/api/agent/v1/probes", panel.base))
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(old.len(), 1);
    assert_eq!(old[0]["enabled"], false);
    assert!(old[0].get("monitor").is_none());
    assert!(old[0].get("execution_authorized").is_none());
    let current: Vec<Value> = panel
        .client
        .get(format!(
            "{}/api/agent/v1/probes?authorization=1",
            panel.base
        ))
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(current[0]["enabled"], false);
    assert_eq!(
        current[0]["monitor"]["authorization"]["identity"]["target"],
        "probe.example.com"
    );
    Ok(())
}

#[sqlx::test]
async fn expiry_revocation_and_retargeting_preserve_history_and_discard_late_samples(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "revocation").await?;
    let spec: Value = panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/probes"),
            &cookie,
            Some(probe_support::authorized(legacy())),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = spec["id"].as_str().unwrap().to_owned();
    let endpoint = format!("{}/api/agent/v1/probe-results", panel.base);
    let issued = probe_support::issued(
        &panel.state.pool,
        &panel.client,
        &panel.base,
        server,
        &ack.session_token,
    )
    .await?;
    let mut sample = json!({"id":Uuid::new_v4(),"probe_id":id,"sampled_at":apsp_protocol::telemetry::now_millis(),"latency_ms":0.0,"loss_percent":0.0,"error":null,"address_family":"ipv4"});
    sample["execution"] = json!(probe_support::execution(&issued, Uuid::parse_str(&id)?)?);
    panel
        .client
        .post(&endpoint)
        .bearer_auth(&ack.session_token)
        .json(&json!({"results":[sample.clone()]}))
        .send()
        .await?
        .error_for_status()?;
    let path = format!("/api/servers/{server}/probes/{id}");
    let mut retargeted = spec.clone();
    retargeted["target"] = json!("different.example.com");
    retargeted["monitor"]["authorization"]["identity"]["target"] = json!("different.example.com");
    assert_eq!(
        panel
            .admin(Method::PATCH, &path, &cookie, Some(retargeted))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let mut revoked = spec.clone();
    revoked["enabled"] = json!(false);
    revoked["monitor"]["authorization"]["enabled"] = json!(false);
    panel
        .admin(Method::PATCH, &path, &cookie, Some(revoked))
        .await?
        .error_for_status()?;
    let mut late = sample;
    late["id"] = json!(Uuid::new_v4());
    panel
        .client
        .post(&endpoint)
        .bearer_auth(&ack.session_token)
        .json(&json!({"results":[late]}))
        .send()
        .await?
        .error_for_status()?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM probe_results WHERE server_id=$1")
            .bind(server)
            .fetch_one(&panel.state.pool)
            .await?,
        1
    );
    let mut expired = spec;
    expired.as_object_mut().unwrap().remove("revision");
    expired
        .as_object_mut()
        .unwrap()
        .remove("execution_authorized");
    expired["monitor"]["authorization"]["expires_at"] = json!(now_timestamp() - 1);
    sqlx::query("UPDATE network_probes SET spec=$2 WHERE id=$1")
        .bind(Uuid::parse_str(&id)?)
        .bind(expired.clone())
        .execute(&panel.state.pool)
        .await?;
    let current: Vec<Value> = panel
        .client
        .get(format!(
            "{}/api/agent/v1/probes?authorization=1",
            panel.base
        ))
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(current[0]["enabled"], false);
    let persisted: Value = sqlx::query_scalar("SELECT spec FROM network_probes WHERE id=$1")
        .bind(Uuid::parse_str(&id)?)
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(persisted, expired);
    Ok(())
}

#[sqlx::test]
async fn public_dashboard_hides_provenance_but_keeps_authorization_and_region(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "public monitor").await?;
    let mut spec = probe_support::authorized(legacy());
    spec["monitor"]["region"] = json!("fixture region");
    spec["monitor"]["authorization"]["source"] = json!("PRIVATE_SOURCE_RECORD");
    spec["monitor"]["authorization"]["scope"] = json!("PRIVATE_SCOPE_RECORD");
    panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/probes"),
            &cookie,
            Some(spec),
        )
        .await?
        .error_for_status()?;
    panel.admin(Method::PATCH,"/api/settings",&cookie,Some(json!({"public_dashboard":true,"offline_alerts":false,"offline_minutes":2,"telegram_enabled":false,"telegram_chat_id":""}))).await?.error_for_status()?;
    let response: Vec<Value> = panel
        .client
        .get(format!(
            "{}/api/dashboard/servers/{server}/probes",
            panel.base
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(response[0]["execution_authorized"], true);
    assert_eq!(response[0]["monitor"]["region"], "fixture region");
    assert_eq!(response[0]["monitor"]["authorization"], Value::Null);
    let encoded = serde_json::to_string(&response)?;
    for secret in [
        "PRIVATE_SOURCE_RECORD",
        "PRIVATE_SCOPE_RECORD",
        "probe.example.com",
    ] {
        assert!(!encoded.contains(secret));
    }
    Ok(())
}

#![forbid(unsafe_code)]

mod business_support;
#[path = "probe_support.rs"]
mod probe_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_panel::probes::ConfiguredProbe;
use apsp_protocol::AgentSettings;
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

fn probe() -> Value {
    probe_support::authorized(json!({
        "id": Uuid::nil(), "name": "连通性", "kind": "tcp", "target": "probe.example.com",
        "port": 443, "interval_secs": 30, "carrier": "测试线路", "enabled": true
    }))
}

async fn stored_counts(pool: &PgPool) -> Result<(i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM servers), (SELECT COUNT(*) FROM network_probes)",
    )
    .fetch_one(pool)
    .await?)
}

#[sqlx::test]
async fn creation_preserves_defaults_and_persists_initial_monitoring(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let legacy = panel.create_server(&cookie, "旧客户端").await?;
    let settings = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{legacy}/agent-settings"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json::<AgentSettings>()
        .await?;
    assert_eq!(settings, AgentSettings::default());
    assert_eq!(stored_counts(&panel.state.pool).await?, (1, 0));

    let settings = AgentSettings {
        sample_interval_secs: 3,
        upload_interval_secs: 10,
        auto_update: true,
        discover_public_ips: false,
    };
    let mut icmp = probe();
    icmp["kind"] = json!("icmp");
    icmp["port"] = Value::Null;
    icmp["target"] = json!("::1");
    icmp = probe_support::authorized(icmp);
    let request =
        json!({"name":"  测试服务器  ","agent_settings":settings,"probes":[probe(),icmp]});
    let unauthorized = panel
        .client
        .post(format!("{}/api/servers", panel.base))
        .json(&request)
        .send()
        .await?;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(stored_counts(&panel.state.pool).await?, (1, 0));
    let response = panel
        .admin(Method::POST, "/api/servers", &cookie, Some(request))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let server: Value = response.json().await?;
    let id = server["id"].as_i64().unwrap();
    assert_eq!(server["name"], "测试服务器");
    assert_eq!(server["online"], false);
    let stored_settings = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{id}/agent-settings"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json::<AgentSettings>()
        .await?;
    assert_eq!(stored_settings, settings);
    let probes = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{id}/probes"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json::<Vec<ConfiguredProbe>>()
        .await?;
    assert_eq!(probes.len(), 2);
    assert_ne!(probes[0].id, probes[1].id);
    assert!(probes.iter().all(|p| p.id != Uuid::nil() && p.valid()));
    assert!(probes.iter().all(|p| p.revision == Some(1)));
    assert!(probes.iter().any(|p| p.target == "::1" && p.port.is_none()));
    panel
        .admin(
            Method::PATCH,
            &format!("/api/servers/{id}"),
            &cookie,
            Some(json!({"name":"重命名"})),
        )
        .await?
        .error_for_status()?;
    let unchanged: Value = sqlx::query_scalar("SELECT agent_settings FROM servers WHERE id=$1")
        .bind(id)
        .fetch_one(&panel.state.pool)
        .await?;
    assert_eq!(unchanged, json!(settings));
    assert_eq!(stored_counts(&panel.state.pool).await?, (2, 2));
    Ok(())
}

#[sqlx::test]
async fn invalid_initial_settings_or_probes_leave_no_server(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let mut invalid_target = probe();
    invalid_target["target"] = json!("https://probe.example.com/path");
    let mut invalid_tcp = probe();
    invalid_tcp["port"] = Value::Null;
    let mut invalid_icmp = probe();
    invalid_icmp["kind"] = json!("icmp");
    let mut invalid_interval = probe();
    invalid_interval["interval_secs"] = json!(9);
    let cases = [
        json!({"name":" "}),
        json!({"name":"settings", "agent_settings":{"sample_interval_secs":0}}),
        json!({"name":"settings", "agent_settings":{"upload_interval_secs":61}}),
        json!({"name":"settings", "agent_settings":{"sample_interval_secs":10,"upload_interval_secs":3}}),
        json!({"name":"target", "probes":[probe(),invalid_target]}),
        json!({"name":"tcp", "probes":[probe(),invalid_tcp]}),
        json!({"name":"icmp", "probes":[probe(),invalid_icmp]}),
        json!({"name":"interval", "probes":[probe(),invalid_interval]}),
        json!({"name":"limit", "probes":vec![probe();33]}),
    ];
    for request in cases {
        let response = panel
            .admin(Method::POST, "/api/servers", &cookie, Some(request.clone()))
            .await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{request}");
        assert_eq!(stored_counts(&panel.state.pool).await?, (0, 0));
    }
    let response = panel
        .admin(
            Method::POST,
            "/api/servers",
            &cookie,
            Some(json!({"name":"limit", "probes":vec![probe();32]})),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let server: Value = response.json().await?;
    assert_eq!(stored_counts(&panel.state.pool).await?, (1, 32));
    let response = panel
        .admin(
            Method::POST,
            &format!("/api/servers/{}/probes", server["id"]),
            &cookie,
            Some(probe()),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    Ok(())
}

#[sqlx::test]
async fn probe_storage_failure_rolls_back_creation_before_retry(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    sqlx::raw_sql(
        "CREATE FUNCTION reject_fixture_probe() RETURNS TRIGGER AS $$
         BEGIN
           IF NEW.spec->>'name' = 'reject-insert' THEN RAISE EXCEPTION 'TEST_ONLY'; END IF;
           RETURN NEW;
         END; $$ LANGUAGE plpgsql;
         CREATE TRIGGER reject_fixture_probe BEFORE INSERT ON network_probes
         FOR EACH ROW EXECUTE FUNCTION reject_fixture_probe();",
    )
    .execute(&panel.state.pool)
    .await?;
    let mut rejected = probe();
    rejected["name"] = json!("reject-insert");
    let response = panel
        .admin(
            Method::POST,
            "/api/servers",
            &cookie,
            Some(json!({"name":"rollback","probes":[probe(),rejected]})),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(stored_counts(&panel.state.pool).await?, (0, 0));
    let response = panel
        .admin(
            Method::POST,
            "/api/servers",
            &cookie,
            Some(json!({"name":"rollback","probes":[probe()]})),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(stored_counts(&panel.state.pool).await?, (1, 1));
    Ok(())
}

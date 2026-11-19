use super::metadata;
use crate::business_support::{TestPanel, id, receive_envelope, send_envelope};
use anyhow::{Context, Result};
use apsp_protocol::{
    ApplyResult, ApplyStatus, Envelope, Heartbeat, SELF_CONTAINED_AGENT_CAPABILITY, now_timestamp,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

async fn supported_online(pool: &PgPool, server: i64) -> Result<()> {
    sqlx::query("UPDATE servers SET capabilities=$2,static_info=$3,last_seen=$4 WHERE id=$1")
        .bind(server)
        .bind(json!([
            SELF_CONTAINED_AGENT_CAPABILITY,
            "singbox",
            apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY,
        ]))
        .bind(json!({
            "arch": "amd64",
            "os": "linux",
            "libc": "gnu",
            "agent_version": "0.4.0",
            "components": { "sing-box": apsp_traffic::RUNTIME_VERSION },
        }))
        .bind(now_timestamp())
        .execute(pool)
        .await?;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn saving_active_node_publishes_automatically_and_removing_last_node_emits_empty_manifest(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, mut socket, ack) = panel
        .authenticated_device(&cookie, "Automatic publication")
        .await?;
    supported_online(&pool, server).await?;
    let initial = metadata(&panel, &cookie, server).await?;
    assert_eq!(initial["configured"], false);
    assert_eq!(initial["status"]["state"], "unconfigured");
    assert_eq!(initial["status"]["target_rev"], 0);
    assert_eq!(initial["status"]["applied_rev"], 0);

    let node = id(&panel.create_node(&cookie, server, "First node").await?)?;
    let queued = metadata(&panel, &cookie, server).await?;
    assert_eq!(queued["configured"], true);
    assert_eq!(queued["source"], "configured");
    assert_eq!(queued["read_only"], false);
    assert_eq!(queued["runtime_version"], Value::Null);
    assert_eq!(queued["status"]["state"], "pending");
    assert_eq!(queued["status"]["target_rev"], 0);

    panel.publish_now().await?;
    assert_eq!(
        receive_envelope(&mut socket).await?.message_type,
        "manifest.changed"
    );
    let pending = metadata(&panel, &cookie, server).await?;
    assert_eq!(pending["status"]["state"], "pending");
    assert_eq!(pending["status"]["target_rev"], 1);
    assert_eq!(pending["status"]["applied_rev"], 0);

    let manifest_url = format!("{}/api/agent/v1/manifest", panel.base);
    let active_manifest: Value = panel
        .client
        .get(&manifest_url)
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(active_manifest["rev"], 1);
    assert_eq!(active_manifest["modules"]["singbox"]["config_rev"], 1);
    assert!(active_manifest["modules"]["singbox"]["artifact"].is_null());

    apsp_traffic::device_config::record_apply_result(
        &panel.state,
        server,
        ApplyResult {
            module: "singbox".into(),
            rev: 1,
            op_id: Uuid::new_v4(),
            status: ApplyStatus::Applied,
            healthy: true,
            error: None,
        },
    )
    .await?;
    assert_eq!(
        metadata(&panel, &cookie, server).await?["status"]["state"],
        "ready"
    );

    // Disabling the last node stops sing-box via an empty desired manifest and reconciles applied=0.
    panel
        .admin(
            Method::PATCH,
            &format!("/api/traffic/nodes/{node}"),
            &cookie,
            Some(json!({"enabled": false})),
        )
        .await?
        .error_for_status()?;
    panel.publish_now().await?;
    assert_eq!(
        receive_envelope(&mut socket).await?.message_type,
        "manifest.changed"
    );
    let stop_manifest: Value = panel
        .client
        .get(&manifest_url)
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(stop_manifest["rev"], 2);
    assert_eq!(stop_manifest["modules"], json!({}));
    assert_eq!(
        panel
            .client
            .get(format!("{}/api/agent/v1/bundles/1", panel.base))
            .bearer_auth(&ack.session_token)
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    send_envelope(
        &mut socket,
        Envelope::new(
            "heartbeat",
            Heartbeat {
                applied: Default::default(),
                uptime_secs: 90,
            },
        )?,
    )
    .await?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let applied: i64 = sqlx::query_scalar(
                "SELECT applied_rev FROM server_module_status WHERE server_id=$1 AND module='singbox'",
            )
            .bind(server)
            .fetch_one(&pool)
            .await?;
            if applied == 0 {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    let stopped = metadata(&panel, &cookie, server).await?;
    assert_eq!(stopped["configured"], false);
    assert_eq!(stopped["status"]["state"], "unconfigured");
    assert_eq!(stopped["status"]["target_rev"], 0);
    assert_eq!(stopped["status"]["applied_rev"], 0);
    assert_eq!(stopped["status"]["current_healthy"], false);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn legacy_or_mismatched_agent_requires_upgrade_and_preserves_device_evidence(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "Upgrade gate").await?;
    panel.create_node(&cookie, server, "Node").await?;
    panel.publish_now().await?;

    sqlx::query("UPDATE servers SET capabilities=$2,last_seen=$3 WHERE id=$1")
        .bind(server)
        .bind(json!([
            "singbox",
            apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY,
        ]))
        .bind(now_timestamp())
        .execute(&pool)
        .await?;
    let manifest_url = format!("{}/api/agent/v1/manifest", panel.base);
    let response = panel
        .client
        .get(&manifest_url)
        .bearer_auth(&ack.session_token)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(
        response.json::<Value>().await?["error"]
            .as_str()
            .context("legacy agent reason")?
            .contains("尚未声明内置 sing-box")
    );
    let upgrade = metadata(&panel, &cookie, server).await?;
    assert_eq!(upgrade["agent_supported"], false);
    assert_eq!(upgrade["status"]["state"], "upgrade_required");

    // Embedded component version mismatch also requires an Agent upgrade.
    sqlx::query("UPDATE servers SET capabilities=$2,static_info=$3,last_seen=$4 WHERE id=$1")
        .bind(server)
        .bind(json!([
            SELF_CONTAINED_AGENT_CAPABILITY,
            "singbox",
            apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY,
        ]))
        .bind(json!({
            "arch": "amd64",
            "os": "linux",
            "components": { "sing-box": "1.14.0-apsp.1" }
        }))
        .bind(now_timestamp())
        .execute(&pool)
        .await?;
    let mismatch = panel
        .client
        .get(&manifest_url)
        .bearer_auth(&ack.session_token)
        .send()
        .await?;
    assert_eq!(mismatch.status(), StatusCode::CONFLICT);
    assert_eq!(
        metadata(&panel, &cookie, server).await?["status"]["state"],
        "upgrade_required"
    );

    supported_online(&pool, server).await?;
    let manifest: Value = panel
        .client
        .get(&manifest_url)
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(manifest["modules"]["singbox"]["artifact"].is_null());
    assert_eq!(
        metadata(&panel, &cookie, server).await?["status"]["state"],
        "pending"
    );

    let failure = ApplyResult {
        module: "singbox".into(),
        rev: 1,
        op_id: Uuid::new_v4(),
        status: ApplyStatus::Failed,
        healthy: false,
        error: Some("TEST_ONLY runtime did not start".into()),
    };
    apsp_traffic::device_config::record_apply_result(&panel.state, server, failure).await?;
    assert_eq!(
        metadata(&panel, &cookie, server).await?["status"]["state"],
        "failed"
    );
    apsp_traffic::device_config::record_apply_result(
        &panel.state,
        server,
        ApplyResult {
            module: "singbox".into(),
            rev: 1,
            op_id: Uuid::new_v4(),
            status: ApplyStatus::Applied,
            healthy: true,
            error: None,
        },
    )
    .await?;
    let ready = metadata(&panel, &cookie, server).await?;
    assert_eq!(ready["status"]["state"], "ready");
    assert_eq!(ready["status"]["current_healthy"], true);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn status_distinguishes_waiting_agent_offline_and_new_target_after_device_error(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "Status transitions").await?;
    let node = id(&panel.create_node(&cookie, server, "Initial node").await?)?;
    panel.publish_now().await?;
    assert_eq!(
        metadata(&panel, &cookie, server).await?["status"]["state"],
        "waiting_agent"
    );

    // Enrolled device that has never connected stays waiting_agent; once seen and offline, offline.
    sqlx::query("UPDATE servers SET device_public_key='TEST_ONLY-enrolled',capabilities=$2,last_seen=NULL WHERE id=$1")
        .bind(server)
        .bind(json!([
            SELF_CONTAINED_AGENT_CAPABILITY,
            "singbox",
            apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY,
        ]))
        .execute(&pool)
        .await?;
    assert_eq!(
        metadata(&panel, &cookie, server).await?["status"]["state"],
        "waiting_agent"
    );
    sqlx::query("UPDATE servers SET last_seen=1 WHERE id=$1")
        .bind(server)
        .execute(&pool)
        .await?;
    assert_eq!(
        metadata(&panel, &cookie, server).await?["status"]["state"],
        "offline"
    );

    supported_online(&pool, server).await?;
    apsp_traffic::device_config::record_apply_result(
        &panel.state,
        server,
        ApplyResult {
            module: "singbox".into(),
            rev: 1,
            op_id: Uuid::new_v4(),
            status: ApplyStatus::Failed,
            healthy: false,
            error: Some("TEST_ONLY original device error".into()),
        },
    )
    .await?;
    assert_eq!(
        metadata(&panel, &cookie, server).await?["status"]["state"],
        "failed"
    );

    // Publishing revision 2 moves status back to pending while preserving last_error from rev 1.
    let user = id(&panel.create_user(&cookie, "New target user").await?)?;
    panel.grant(&cookie, user, node).await?;
    panel.publish_now().await?;
    let pending = metadata(&panel, &cookie, server).await?;
    assert_eq!(pending["status"]["target_rev"], 2);
    assert_eq!(pending["status"]["state"], "pending");
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT last_error FROM server_module_status WHERE server_id=$1 AND module='singbox'"
        )
        .bind(server)
        .fetch_one(&pool)
        .await?,
        Some("TEST_ONLY original device error".into())
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn authenticated_heartbeat_recovers_runtime_status_after_lost_result_and_old_failure(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    for prior_failure in [false, true] {
        let (server, mut socket, _ack) = panel
            .authenticated_device(&cookie, "Checkpoint recovery")
            .await?;
        supported_online(&pool, server).await?;
        panel.create_node(&cookie, server, "Recovery node").await?;
        panel.publish_now().await?;
        assert_eq!(
            receive_envelope(&mut socket).await?.message_type,
            "manifest.changed"
        );
        if prior_failure {
            apsp_traffic::device_config::record_apply_result(
                &panel.state,
                server,
                ApplyResult {
                    module: "singbox".into(),
                    rev: 1,
                    op_id: Uuid::new_v4(),
                    status: ApplyStatus::Failed,
                    healthy: false,
                    error: Some("TEST_ONLY previous failed attempt".into()),
                },
            )
            .await?;
        }
        send_envelope(
            &mut socket,
            Envelope::new(
                "heartbeat",
                Heartbeat {
                    applied: [("singbox".into(), 1)].into(),
                    uptime_secs: 60,
                },
            )?,
        )
        .await?;
        // This hint is emitted only after the authenticated checkpoint commits.
        assert_eq!(
            receive_envelope(&mut socket).await?.message_type,
            "manifest.changed"
        );
        let evidence: (i64, i64, bool, Option<String>) = sqlx::query_as("SELECT applied_rev,last_result_rev,healthy,last_error FROM server_module_status WHERE server_id=$1 AND module='singbox'")
            .bind(server).fetch_one(&pool).await?;
        assert_eq!(evidence.0, 1);
        assert_eq!(evidence.1, if prior_failure { 1 } else { 0 });
        assert!(evidence.2);
        assert_eq!(
            evidence.3,
            prior_failure.then(|| "TEST_ONLY previous failed attempt".into())
        );
        assert_eq!(
            metadata(&panel, &cookie, server).await?["status"]["state"],
            "ready"
        );
    }
    Ok(())
}

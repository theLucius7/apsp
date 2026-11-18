#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result, ensure};
use apsp_access::entitlements;
use apsp_protocol::{
    ApplyResult, ApplyStatus, Bundle, Hello, Message, PROTOCOL_VERSION,
    RUNTIME_CHECKPOINT_CAPABILITY, RuntimeCheckpoint, RuntimeCheckpointRequest,
    RuntimeCheckpointResult, SELF_CONTAINED_AGENT_CAPABILITY, UsageBatch, UsageRecord,
    now_timestamp,
};
use apsp_resources::device_channel;
use apsp_traffic::{publisher, runtime_control};
use business_support::{
    TestPanel,
    deployment::{RuntimeObservation, observe, observe_runtime},
    id,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

async fn scenario(panel: &TestPanel, cookie: &str) -> Result<(i64, i64, i64)> {
    let server = panel.create_server(cookie, "Deployment gate").await?;
    let node = id(&panel
        .create_node(cookie, server, "Ordinary listener")
        .await?)?;
    let account = id(&panel.create_user(cookie, "Authorized account").await?)?;
    panel.grant(cookie, account, node).await?;
    Ok((server, node, account))
}

async fn due(panel: &TestPanel, server: i64) -> Result<()> {
    sqlx::query(
        "UPDATE runtime_publications SET dirty_at=0 WHERE server_id=$1 AND dirty_at IS NOT NULL",
    )
    .bind(server)
    .execute(&panel.state.pool)
    .await?;
    publisher::publish_due(&panel.state, None).await
}

async fn revision(pool: &PgPool, server: i64) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(MAX(rev),0) FROM deployments WHERE server_id=$1 AND module='singbox'",
    )
    .bind(server)
    .fetch_one(pool)
    .await?)
}

async fn pending(pool: &PgPool, server: i64) -> Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_publications WHERE server_id=$1 AND dirty_at IS NOT NULL)")
            .bind(server)
            .fetch_one(pool)
            .await?,
    )
}

async fn configuration(pool: &PgPool, server: i64) -> Result<Value> {
    let bytes: String = sqlx::query_scalar("SELECT bundle FROM deployments WHERE server_id=$1 AND module='singbox' ORDER BY rev DESC LIMIT 1")
        .bind(server).fetch_one(pool).await?;
    let bundle: Bundle = serde_json::from_str(&bytes)?;
    Ok(serde_json::from_str(
        bundle
            .files
            .get("config.json")
            .context("native configuration")?,
    )?)
}

async fn check_entry(panel: &TestPanel, cookie: &str, server: i64, key: &str) -> Result<Value> {
    let view: Value = panel
        .admin(
            Method::GET,
            &format!("/api/traffic/servers/{server}/operations-view"),
            cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    view["preflight"]["checks"]
        .as_array()
        .and_then(|checks| checks.iter().find(|check| check["key"] == key))
        .cloned()
        .with_context(|| format!("preflight check {key}"))
}

#[sqlx::test(migrations = "./migrations")]
async fn first_listener_publishes_automatically_while_preflight_remains_read_only_inspection(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _, _) = scenario(&panel, &cookie).await?;

    // Retired confirmation and empty-bootstrap routes are gone.
    for retired in ["confirm", "bootstrap"] {
        assert_eq!(
            panel
                .admin(
                    Method::POST,
                    &format!("/api/traffic/servers/{server}/operations-view/preflight/{retired}"),
                    &cookie,
                    Some(json!({"id": Uuid::new_v4(), "confirm": true})),
                )
                .await?
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    let before_view: Value = panel
        .admin(
            Method::GET,
            &format!("/api/traffic/servers/{server}/operations-view"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(before_view["preflight"].get("bootstrap").is_none());
    assert!(
        before_view["preflight"]["application"]
            .get("requires_preflight")
            .is_none()
    );
    assert_eq!(
        before_view["preflight"]["application"]["pending_publish"],
        true
    );

    // Saving an ordinary listener compiles and publishes automatically without confirmation.
    due(&panel, server).await?;
    assert_eq!(revision(&pool, server).await?, 1);
    assert!(!pending(&pool, server).await?);
    assert_eq!(
        configuration(&pool, server).await?["inbounds"]
            .as_array()
            .context("inbounds")?
            .len(),
        1
    );

    // Read-only preflight inspection still checks live device evidence and expiry.
    let observed = observe(&panel, &cookie, server).await?;
    let snapshot = check_entry(&panel, &cookie, server, "snapshot").await?;
    assert_eq!(snapshot["state"], "passed");
    sqlx::query("UPDATE singbox_deployment_preflights SET expires_at=$2 WHERE id=$1")
        .bind(observed)
        .bind(now_timestamp() - 1)
        .execute(&pool)
        .await?;
    let expired = check_entry(&panel, &cookie, server, "snapshot").await?;
    assert_eq!(expired["state"], "unknown");
    assert_eq!(expired["evidence"]["unexpired"], false);
    Ok(())
}

/// The publication transaction reads in a REPEATABLE READ snapshot. The device channel writes
/// the server row (`last_seen`) on every message; such writes committed after the snapshot must
/// neither fail the publication with a serialization error nor wait for it.
#[sqlx::test(migrations = "./migrations")]
async fn publication_tolerates_contact_updates_after_its_snapshot(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _, _) = scenario(&panel, &cookie).await?;
    sqlx::query("UPDATE runtime_publications SET dirty_at=0 WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await?;

    let mut gate = pool.begin().await?;
    apsp_traffic::business::lock_publication(&mut gate).await?;
    let publication = tokio::spawn({
        let state = panel.state.clone();
        async move { publisher::publish_due(&state, None).await }
    });
    let waiting = "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND objid=73201001 AND NOT granted)";
    let deadline = Instant::now() + Duration::from_secs(30);
    while !sqlx::query_scalar::<_, bool>(waiting)
        .fetch_one(&pool)
        .await?
    {
        ensure!(
            Instant::now() < deadline,
            "the publisher waits for the publication lock"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // The publisher's snapshot is taken; a device message updates the server row after it.
    let contact = "UPDATE servers SET last_seen=$2,last_contact_at=$2 WHERE id=$1";
    sqlx::query(contact)
        .bind(server)
        .bind(now_timestamp())
        .execute(&pool)
        .await?;
    let stop = Arc::new(AtomicBool::new(false));
    let writer = tokio::spawn({
        let (pool, stop) = (pool.clone(), stop.clone());
        async move {
            let mut updates = 0_u64;
            while !stop.load(Ordering::Relaxed) {
                sqlx::query(contact)
                    .bind(server)
                    .bind(now_timestamp())
                    .execute(&pool)
                    .await?;
                updates += 1;
            }
            anyhow::Ok(updates)
        }
    });
    gate.commit().await?;
    let published = publication.await?;
    stop.store(true, Ordering::Relaxed);
    let updates = writer.await??;
    published?;
    ensure!(
        updates > 0,
        "the device kept writing during the publication"
    );
    assert_eq!(revision(&pool, server).await?, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM deployments WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?,
        1
    );
    assert!(!pending(&pool, server).await?);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn same_second_requests_use_submission_order_and_detect_material_changes(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, node, _) = scenario(&panel, &cookie).await?;
    due(&panel, server).await?;
    assert_eq!(revision(&pool, server).await?, 1);

    let first = observe(&panel, &cookie, server).await?;
    let common_second: i64 =
        sqlx::query_scalar("SELECT created_at FROM singbox_deployment_preflights WHERE id=$1")
            .bind(first)
            .fetch_one(&pool)
            .await?;
    let second = observe(&panel, &cookie, server).await?;
    let older_large_uuid = Uuid::from_u128(u128::MAX);
    ensure!(second < older_large_uuid, "controlled UUID ordering");
    sqlx::query("UPDATE singbox_deployment_preflights SET id=$2,created_at=$3 WHERE id=$1")
        .bind(first)
        .bind(older_large_uuid)
        .bind(common_second)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE singbox_deployment_preflights SET created_at=$2 WHERE id=$1")
        .bind(second)
        .bind(common_second)
        .execute(&pool)
        .await?;
    let view: Value = panel
        .admin(
            Method::GET,
            &format!("/api/traffic/servers/{server}/operations-view"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(view["preflight"]["id"], json!(second));
    assert_eq!(
        check_entry(&panel, &cookie, server, "snapshot").await?["state"],
        "passed"
    );

    // Material listener edits invalidate snapshot freshness while still publishing automatically.
    panel
        .admin(
            Method::PATCH,
            &format!("/api/traffic/nodes/{node}"),
            &cookie,
            Some(json!({"sni":"next.example.com"})),
        )
        .await?
        .error_for_status()?;
    let drifted = check_entry(&panel, &cookie, server, "snapshot").await?;
    assert_eq!(drifted["state"], "unknown");
    assert_eq!(drifted["evidence"]["configuration_matches"], false);
    due(&panel, server).await?;
    assert_eq!(revision(&pool, server).await?, 2);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn same_listener_credentials_quota_and_final_revocation_publish_without_preflight(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, node, account) = scenario(&panel, &cookie).await?;
    let survivor = id(&panel.create_user(&cookie, "Surviving account").await?)?;
    panel.grant(&cookie, survivor, node).await?;
    let old: String =
        sqlx::query_scalar("SELECT uuid::text FROM accesses WHERE user_id=$1 AND node_id=$2")
            .bind(account)
            .bind(node)
            .fetch_one(&pool)
            .await?;
    due(&panel, server).await?;
    assert_eq!(revision(&pool, server).await?, 1);
    panel.revoke(&cookie, account, node).await?;
    let replacement = panel.grant(&cookie, account, node).await?;
    let replacement_uuid = replacement["uuid"]
        .as_str()
        .context("new connection UUID")?;
    assert_ne!(replacement_uuid, old);
    due(&panel, server).await?;
    assert_eq!(revision(&pool, server).await?, 2);
    let native = configuration(&pool, server).await?;
    assert!(native.to_string().contains(replacement_uuid));
    assert!(!native.to_string().contains(&old));

    let plan: Value = panel
        .admin(
            Method::POST,
            "/api/access/package-groups",
            &cookie,
            Some(json!({
                "name":"Bounded quota","monthly_bytes":"10","reset_day":1,"reset_hour":0,
                "reset_minute":0,"timezone":"UTC","duration_days":365,
            })),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let assignment: Value = panel
        .admin(
            Method::POST,
            &format!("/api/access/users/{account}/package"),
            &cookie,
            Some(json!({
                "package_group_id":id(&plan)?,"request_id":Uuid::new_v4(),
            })),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let starts = assignment["starts_at"].as_i64().context("package starts")?;
    apsp_access::usage::ingest(
        &panel.state,
        server,
        UsageBatch {
            epoch: Uuid::new_v4(),
            seq: 1,
            period_start: starts,
            period_end: starts + 1,
            records: vec![UsageRecord {
                stat_name: format!("u{account}_n{node}"),
                uplink: 10,
                downlink: 0,
            }],
        },
    )
    .await?;
    entitlements::refresh(&pool, now_timestamp()).await?;
    due(&panel, server).await?;
    assert_eq!(revision(&pool, server).await?, 3);
    let native = configuration(&pool, server).await?;
    assert!(!native.to_string().contains(replacement_uuid));
    assert!(native.to_string().contains(&format!("u{survivor}_n{node}")));
    panel.revoke(&cookie, survivor, node).await?;
    due(&panel, server).await?;
    assert_eq!(revision(&pool, server).await?, 4);
    assert!(
        configuration(&pool, server).await?["inbounds"]
            .as_array()
            .context("inbounds")?
            .is_empty()
    );
    let preflights: i64 =
        sqlx::query_scalar("SELECT count(*) FROM singbox_deployment_preflights WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        preflights, 0,
        "automatic publication never manufactures preflight records"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn runtime_listener_is_owned_once_its_checkpoint_is_answered(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _, _) = scenario(&panel, &cookie).await?;
    due(&panel, server).await?;
    assert_eq!(revision(&pool, server).await?, 1);

    // The Agent certifies exact runtime instances, so revision 1 is applied only
    // through a checkpoint. Its statistics listener occupies a port preflight inspects.
    device_channel::process_message(
        &panel.state,
        server,
        Message::Hello(Hello {
            agent_version: "TEST_ONLY".into(),
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec![
                SELF_CONTAINED_AGENT_CAPABILITY.into(),
                "singbox".into(),
                RUNTIME_CHECKPOINT_CAPABILITY.into(),
            ],
            applied: Default::default(),
        }),
    )
    .await?;
    let running = || RuntimeObservation {
        listener_pid: 4242,
        managed_pid: 4242,
    };
    observe_runtime(&panel, &cookie, server, running()).await?;
    assert_eq!(
        check_entry(&panel, &cookie, server, "ports").await?["state"],
        "failed",
        "a listener is not recognized as the managed runtime's before a checkpoint proves its deployment"
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
    // Unanswered checkpoint keeps status in pending rather than claiming ready or failed.
    let status: Value = panel
        .admin(
            Method::GET,
            &format!("/api/traffic/servers/{server}"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        status["status"]["state"], "pending",
        "an applied revision awaiting certification is not a failed runtime"
    );

    let payload: Value = sqlx::query_scalar(
        "SELECT request_json FROM runtime_control_requests WHERE server_id=$1 AND kind='checkpoint' AND state='pending'",
    )
    .bind(server)
    .fetch_one(&pool)
    .await?;
    let request: RuntimeCheckpointRequest = serde_json::from_value(payload)?;
    runtime_control::record_checkpoint_result(
        &panel.state,
        server,
        RuntimeCheckpointResult {
            request_id: request.request_id,
            request_digest: request.digest()?,
            observed: Some(RuntimeCheckpoint {
                binding: request.expected.clone(),
                activation_id: Uuid::from_u128(100),
                instance_id: "backend:TEST_ONLY-instance".into(),
                healthy: true,
            }),
            success: true,
            error: None,
        },
    )
    .await?;

    let certified: Value = panel
        .admin(
            Method::GET,
            &format!("/api/traffic/servers/{server}"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(certified["status"]["state"], "ready");

    observe_runtime(
        &panel,
        &cookie,
        server,
        RuntimeObservation {
            listener_pid: 5151,
            managed_pid: 4242,
        },
    )
    .await?;
    assert_eq!(
        check_entry(&panel, &cookie, server, "ports").await?["state"],
        "failed",
        "another process on the runtime port is still a conflict"
    );

    observe_runtime(&panel, &cookie, server, running()).await?;
    let check = check_entry(&panel, &cookie, server, "ports").await?;
    assert_eq!(check["state"], "passed");
    assert_eq!(check["evidence"]["verified_managed_pid"], 4242);
    Ok(())
}

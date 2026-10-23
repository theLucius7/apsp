#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result};
use apsp_protocol::{
    AuthChallenge, AuthResponse, Envelope, Hello, PROTOCOL_VERSION, RETIREMENT_CAPABILITY,
    RetirementReceipt, RetirementRequest, RetirementResult, UsageBatch, UsageRecord,
    retirement_receipt_message,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use business_support::{Socket, TestPanel, id, receive_envelope, send_envelope};
use ed25519_dalek::{Signer, SigningKey};
use reqwest::{Method, StatusCode, header};
use sqlx::PgPool;
use std::{collections::BTreeMap, time::Duration};
use tokio::time::timeout;
use uuid::Uuid;

fn receipt(key: &SigningKey, server_id: i64, request_id: Uuid) -> RetirementReceipt {
    RetirementReceipt {
        server_id,
        request_id,
        signature: URL_SAFE_NO_PAD.encode(
            key.sign(&retirement_receipt_message(server_id, request_id))
                .to_bytes(),
        ),
    }
}

async fn enable_retirement(panel: &TestPanel, socket: &mut Socket, server: i64) -> Result<()> {
    send_envelope(
        socket,
        Envelope::new(
            "hello",
            Hello {
                agent_version: "retirement-test".into(),
                protocol_version: PROTOCOL_VERSION,
                capabilities: vec![RETIREMENT_CAPABILITY.into()],
                applied: BTreeMap::new(),
            },
        )?,
    )
    .await?;
    timeout(Duration::from_secs(5), async {
        loop {
            let supported: bool =
                sqlx::query_scalar("SELECT capabilities ? $2 FROM servers WHERE id=$1")
                    .bind(server)
                    .bind(RETIREMENT_CAPABILITY)
                    .fetch_one(&panel.state.pool)
                    .await?;
            if supported {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    Ok(())
}

async fn request(socket: &mut Socket) -> Result<RetirementRequest> {
    loop {
        let envelope = receive_envelope(socket).await?;
        if envelope.message_type == "retirement.request" {
            return Ok(envelope.to_payload()?);
        }
        anyhow::ensure!(
            ["manifest.changed", "usage.ack"].contains(&envelope.message_type.as_str()),
            "unexpected notification"
        );
    }
}

async fn active(pool: &PgPool, server: i64) -> Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT deleted_at IS NULL FROM servers WHERE id=$1")
            .bind(server)
            .fetch_one(pool)
            .await?,
    )
}

#[sqlx::test]
async fn online_delete_waits_for_signed_receipt_and_preserves_usage(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, mut socket, _, key) = panel
        .authenticated_device_with_key(&cookie, "retire-online")
        .await?;
    enable_retirement(&panel, &mut socket, server).await?;
    let node = id(&panel.create_node(&cookie, server, "node").await?)?;
    let user = id(&panel.create_user(&cookie, "user").await?)?;
    panel.grant(&cookie, user, node).await?;
    panel.publish_now().await?;
    apsp_panel::usage::ingest(
        &panel.state,
        server,
        UsageBatch {
            epoch: Uuid::new_v4(),
            seq: 1,
            period_start: 100,
            period_end: 130,
            records: vec![UsageRecord {
                stat_name: format!("u{user}_n{node}"),
                uplink: 100,
                downlink: 200,
            }],
        },
    )
    .await?;
    panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/enrollment"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?;
    let client = panel.client.clone();
    let url = format!("{}/api/servers/{server}", panel.base);
    let deletion = tokio::spawn(async move {
        client
            .delete(url)
            .header(header::COOKIE, cookie)
            .send()
            .await
    });
    let command = request(&mut socket).await?;
    assert!(
        active(&pool, server).await?,
        "online server must remain until cleanup is confirmed"
    );
    let proof = receipt(&key, server, command.request_id);
    send_envelope(
        &mut socket,
        Envelope::new(
            "retirement.result",
            RetirementResult {
                request_id: command.request_id,
                success: true,
                error: None,
                receipt: Some(proof.clone()),
            },
        )?,
    )
    .await?;
    assert_eq!(deletion.await??.status(), StatusCode::NO_CONTENT);
    assert!(!active(&pool, server).await?);
    for table in ["sessions", "enrollment_tokens"] {
        let count: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE server_id=$1"))
                .bind(server)
                .fetch_one(&pool)
                .await?;
        assert_eq!(count, 0);
    }
    let total: String = sqlx::query_scalar(
        "SELECT SUM(uplink+downlink)::text FROM usage_records WHERE server_id=$1",
    )
    .bind(server)
    .fetch_one(&pool)
    .await?;
    assert_eq!(total, "300");
    for _ in 0..2 {
        assert_eq!(
            panel
                .client
                .post(format!("{}/api/agent/v1/retirement/receipt", panel.base))
                .json(&proof)
                .send()
                .await?
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    let mut wrong = proof;
    wrong.request_id = Uuid::new_v4();
    assert_eq!(
        panel
            .client
            .post(format!("{}/api/agent/v1/retirement/receipt", panel.base))
            .json(&wrong)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[sqlx::test]
async fn failed_retirement_keeps_server_and_offline_retry_is_not_confirmation(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, mut socket, _, key) = panel
        .authenticated_device_with_key(&cookie, "retire-failure")
        .await?;
    enable_retirement(&panel, &mut socket, server).await?;
    let mut original = None;
    for _ in 0..2 {
        let client = panel.client.clone();
        let url = format!("{}/api/servers/{server}", panel.base);
        let cookie = cookie.clone();
        let deletion = tokio::spawn(async move {
            client
                .delete(url)
                .header(header::COOKIE, cookie)
                .send()
                .await
        });
        let command = request(&mut socket).await?;
        if let Some(previous) = original {
            assert_eq!(previous, command.request_id);
        }
        original = Some(command.request_id);
        let untrusted = SigningKey::generate(&mut rand::rngs::OsRng);
        assert_eq!(
            panel
                .client
                .post(format!("{}/api/agent/v1/retirement/receipt", panel.base))
                .json(&receipt(&untrusted, server, command.request_id))
                .send()
                .await?
                .status(),
            StatusCode::UNAUTHORIZED
        );
        send_envelope(
            &mut socket,
            Envelope::new(
                "retirement.result",
                RetirementResult {
                    request_id: command.request_id,
                    success: false,
                    error: Some("停止服务失败".into()),
                    receipt: None,
                },
            )?,
        )
        .await?;
        let response = deletion.await??;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(response.text().await?.contains("停止服务失败"));
        assert!(active(&pool, server).await?);
    }
    socket.close(None).await?;
    timeout(Duration::from_secs(5), async {
        while panel.state.connections.read().await.contains_key(&server) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    assert_eq!(
        panel
            .admin(
                Method::DELETE,
                &format!("/api/servers/{server}"),
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    let status: String =
        sqlx::query_scalar("SELECT status FROM server_retirements WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?;
    assert_eq!(status, "offline_unconfirmed");
    assert_eq!(
        panel
            .client
            .post(format!("{}/api/agent/v1/retirement/receipt", panel.base))
            .json(&receipt(&key, server, original.context("request")?))
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    let status: String =
        sqlx::query_scalar("SELECT status FROM server_retirements WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await?;
    assert_eq!(status, "confirmed");
    Ok(())
}

#[sqlx::test]
async fn offline_delete_and_unsupported_online_agent_remain_distinct(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let offline = panel.create_server(&cookie, "offline").await?;
    assert_eq!(
        panel
            .client
            .delete(format!("{}/api/servers/{offline}", panel.base))
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        panel
            .admin(
                Method::DELETE,
                &format!("/api/servers/{offline}"),
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        panel
            .admin(
                Method::DELETE,
                &format!("/api/servers/{offline}"),
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    let (online, _socket, _) = panel.authenticated_device(&cookie, "legacy-online").await?;
    let response = panel
        .admin(
            Method::DELETE,
            &format!("/api/servers/{online}"),
            &cookie,
            None,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(response.text().await?.contains("升级"));
    assert!(active(&pool, online).await?);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM server_retirements")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 0);
    let unknown = receipt(
        &SigningKey::generate(&mut rand::rngs::OsRng),
        offline,
        Uuid::new_v4(),
    );
    assert_eq!(
        panel
            .client
            .post(format!("{}/api/agent/v1/retirement/receipt", panel.base))
            .json(&unknown)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[sqlx::test]
async fn deletion_serializes_with_reconnect_and_cannot_leave_a_live_session(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool.clone()).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "reconnect-race").await?;
    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    sqlx::query("UPDATE servers SET device_public_key=$2 WHERE id=$1")
        .bind(server)
        .bind(URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes()))
        .execute(&pool)
        .await?;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!(
        "{}/api/agent/v1/ws",
        panel.base.replacen("http", "ws", 1)
    ))
    .await?;
    let challenge: AuthChallenge = receive_envelope(&mut socket).await?.to_payload()?;

    // Hold the row so DELETE has begun but cannot commit before authentication arrives.
    let mut blocker = pool.begin().await?;
    sqlx::query("SELECT id FROM servers WHERE id=$1 FOR NO KEY UPDATE")
        .bind(server)
        .fetch_one(&mut *blocker)
        .await?;
    let client = panel.client.clone();
    let url = format!("{}/api/servers/{server}", panel.base);
    let deletion = tokio::spawn(async move {
        client
            .delete(url)
            .header(header::COOKIE, cookie)
            .send()
            .await
    });
    timeout(Duration::from_secs(5), async {
        loop {
            if panel.state.device_lifecycle.try_lock().is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    send_envelope(
        &mut socket,
        Envelope::new(
            "auth.response",
            AuthResponse {
                server_id: server,
                signature: URL_SAFE_NO_PAD.encode(key.sign(challenge.nonce.as_bytes()).to_bytes()),
            },
        )?,
    )
    .await?;
    blocker.commit().await?;
    assert_eq!(deletion.await??.status(), StatusCode::NO_CONTENT);
    assert!(
        timeout(Duration::from_secs(5), receive_envelope(&mut socket))
            .await?
            .is_err(),
        "a reconnect racing with deletion must not receive hello.ack"
    );
    assert!(!active(&pool, server).await?);
    assert!(!panel.state.connections.read().await.contains_key(&server));
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE server_id=$1")
        .bind(server)
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        sessions, 0,
        "deleted devices must not retain a newly created session"
    );
    Ok(())
}

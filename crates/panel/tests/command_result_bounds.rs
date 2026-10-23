#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_protocol::{
    CommandClaim, CommandControl, CommandResult, CommandStatus, RemoteCommand, TaskAck,
    now_timestamp,
};
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test]
async fn escaped_bounded_output_is_accepted_while_decoded_overflow_is_rejected(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "escaped output")
        .await?;
    sqlx::query("UPDATE servers SET capabilities=$2 WHERE id=$1")
        .bind(server)
        .bind(json!(["command:execute", "command:lifecycle:v1"]))
        .execute(&panel.state.pool)
        .await?;
    let command: RemoteCommand = panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/commands"),
            &cookie,
            Some(json!({"command":"printf fixture", "timeout_secs":5, "ttl_secs":60})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let endpoint = format!("{}/api/agent/v1/commands/{}", panel.base, command.id);
    let claimed: CommandControl = panel
        .client
        .post(format!("{endpoint}/claim"))
        .bearer_auth(&ack.session_token)
        .json(&CommandClaim {
            claim_id: Uuid::new_v4(),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let maximum = 256 * 1024;
    let result = CommandResult {
        id: command.id,
        status: CommandStatus::Succeeded,
        finished_at: now_timestamp().max(claimed.claimed_at.unwrap()),
        stdout: "\u{1}".repeat(maximum),
        stderr: "\u{7}".repeat(maximum),
        timed_out: false,
        truncated: true,
    };
    let wire = serde_json::to_vec(&result)?;
    assert!(wire.len() > 3 * 1024 * 1024 && wire.len() < 4 * 1024 * 1024);
    for overflow_stdout in [true, false] {
        let mut invalid = result.clone();
        if overflow_stdout {
            invalid.stdout.push('x');
        } else {
            invalid.stderr.push('x');
        }
        assert_eq!(
            panel
                .client
                .post(&endpoint)
                .bearer_auth(&ack.session_token)
                .json(&invalid)
                .send()
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    for _ in 0..2 {
        let received: TaskAck = panel
            .client
            .post(&endpoint)
            .bearer_auth(&ack.session_token)
            .json(&result)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(received.ids, [command.id]);
    }
    let sizes: (i32,i32) = sqlx::query_as("SELECT octet_length(result->>'stdout'),octet_length(result->>'stderr') FROM remote_commands WHERE id=$1")
        .bind(command.id).fetch_one(&panel.state.pool).await?;
    assert_eq!(sizes, (maximum as i32, maximum as i32));
    Ok(())
}

#[sqlx::test]
async fn legacy_nul_output_has_a_bounded_display_projection_and_an_immutable_original_digest(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "legacy binary output")
        .await?;
    sqlx::query("UPDATE servers SET capabilities=$2 WHERE id=$1")
        .bind(server)
        .bind(json!(["command:execute"]))
        .execute(&panel.state.pool)
        .await?;
    let command: RemoteCommand = panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/commands"),
            &cookie,
            Some(json!({"command":"printf fixture", "timeout_secs":5, "ttl_secs":60})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    let pending: Vec<RemoteCommand> = panel
        .client
        .get(format!("{}/api/agent/v1/commands", panel.base))
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(pending.as_slice(), std::slice::from_ref(&command));
    let endpoint = format!("{}/api/agent/v1/commands/{}", panel.base, command.id);
    let mut result = CommandResult {
        id: command.id,
        status: CommandStatus::Succeeded,
        finished_at: now_timestamp(),
        stdout: "\0".repeat(256 * 1024),
        stderr: "before\0after".into(),
        timed_out: false,
        truncated: false,
    };
    for _ in 0..2 {
        let received: TaskAck = panel
            .client
            .post(&endpoint)
            .bearer_auth(&ack.session_token)
            .json(&result)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(received.ids, [command.id]);
    }
    let value: serde_json::Value =
        sqlx::query_scalar("SELECT result FROM remote_commands WHERE id=$1")
            .bind(command.id)
            .fetch_one(&panel.state.pool)
            .await?;
    let saved: CommandResult = serde_json::from_value(value)?;
    assert_eq!(saved.status, result.status);
    assert_eq!(saved.finished_at, result.finished_at);
    assert!(saved.truncated && !saved.timed_out);
    assert!(!saved.stdout.contains('\0') && saved.stdout.len() <= 256 * 1024);
    assert_eq!(saved.stdout, "\u{fffd}".repeat((256 * 1024) / 3));
    assert_eq!(saved.stderr, "before\u{fffd}after");
    // Distinct input must not replay as the same terminal event, even if its
    // display would normalize to the same text.
    result.stderr = "before\u{fffd}after".into();
    assert_eq!(
        panel
            .client
            .post(&endpoint)
            .bearer_auth(&ack.session_token)
            .json(&result)
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    Ok(())
}

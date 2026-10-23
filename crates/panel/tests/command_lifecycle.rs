#![forbid(unsafe_code)]
mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_protocol::{
    CommandClaim, CommandControl, CommandResult, CommandStarted, CommandState, CommandStatus,
    RemoteCommand, now_timestamp,
};
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

async fn create(panel: &TestPanel, cookie: &str, server: i64) -> Result<RemoteCommand> {
    Ok(panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/commands"),
            cookie,
            Some(json!({"command":"printf fixture","timeout_secs":5,"ttl_secs":60})),
        )
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn claim(panel: &TestPanel, token: &str, id: Uuid, claim_id: Uuid) -> Result<CommandControl> {
    Ok(panel
        .client
        .post(format!("{}/api/agent/v1/commands/{id}/claim", panel.base))
        .bearer_auth(token)
        .json(&CommandClaim { claim_id })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn cancel(panel: &TestPanel, cookie: &str, server: i64, id: Uuid) -> Result<CommandControl> {
    Ok(panel
        .admin(
            Method::POST,
            &format!("/api/servers/{server}/commands/{id}/cancel"),
            cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn capabilities(panel: &TestPanel, server: i64, values: Value) -> Result<()> {
    sqlx::query("UPDATE servers SET capabilities=$2 WHERE id=$1")
        .bind(server)
        .bind(values)
        .execute(&panel.state.pool)
        .await?;
    Ok(())
}

#[sqlx::test]
async fn queue_cancellation_and_claim_are_serialized_and_never_reach_legacy_delivery(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "command cancellation")
        .await?;
    capabilities(
        &panel,
        server,
        json!([
            "command:execute",
            "command:lifecycle:v1",
            "command:cancel:v1"
        ]),
    )
    .await?;
    for _ in 0..12 {
        let command = create(&panel, &cookie, server).await?;
        let token = Uuid::new_v4();
        let (cancelled, claimed) = tokio::try_join!(
            cancel(&panel, &cookie, server, command.id),
            claim(&panel, &ack.session_token, command.id, token)
        )?;
        assert!(matches!(
            (cancelled.state, claimed.state),
            (CommandState::Cancelled, CommandState::Cancelled)
                | (CommandState::CancelRequested, CommandState::Claimed)
        ));
        let repeated = cancel(&panel, &cookie, server, command.id).await?;
        assert_eq!(repeated.cancel_requested_at, cancelled.cancel_requested_at);
        assert_eq!(repeated.state, cancelled.state);
    }
    for path in ["commands", "commands/lifecycle"] {
        let pending: Vec<RemoteCommand> = panel
            .client
            .get(format!("{}/api/agent/v1/{path}", panel.base))
            .bearer_auth(&ack.session_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert!(pending.is_empty());
    }
    let command = create(&panel, &cookie, server).await?;
    assert_eq!(
        cancel(&panel, &cookie, server, command.id).await?.state,
        CommandState::Cancelled
    );
    assert_eq!(
        claim(&panel, &ack.session_token, command.id, Uuid::new_v4())
            .await?
            .state,
        CommandState::Cancelled
    );
    let fabricated = CommandResult {
        id: command.id,
        status: CommandStatus::Succeeded,
        finished_at: now_timestamp(),
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        truncated: false,
    };
    assert_eq!(
        panel
            .client
            .post(format!(
                "{}/api/agent/v1/commands/{}",
                panel.base, command.id
            ))
            .bearer_auth(&ack.session_token)
            .json(&fabricated)
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    Ok(())
}

#[sqlx::test]
async fn durable_start_and_terminal_messages_are_idempotent_and_order_independent(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "durable lifecycle")
        .await?;
    let (_, _other_socket, other) = panel
        .authenticated_device(&cookie, "other lifecycle")
        .await?;
    capabilities(
        &panel,
        server,
        json!([
            "command:execute",
            "command:lifecycle:v1",
            "command:cancel:v1"
        ]),
    )
    .await?;
    for cancelling in [true, false] {
        let command = create(&panel, &cookie, server).await?;
        let token = Uuid::new_v4();
        let claimed = claim(&panel, &ack.session_token, command.id, token).await?;
        assert_eq!(claimed.state, CommandState::Claimed);
        assert_eq!(
            claim(&panel, &ack.session_token, command.id, token)
                .await?
                .claimed_at,
            claimed.claimed_at
        );
        let started = CommandStarted {
            claim_id: token,
            started_at: now_timestamp(),
        };
        let endpoint = format!("{}/api/agent/v1/commands/{}", panel.base, command.id);
        assert_eq!(
            panel
                .client
                .post(format!("{endpoint}/started"))
                .bearer_auth(&other.session_token)
                .json(&started)
                .send()
                .await?
                .status(),
            StatusCode::NOT_FOUND
        );
        if cancelling {
            assert_eq!(
                cancel(&panel, &cookie, server, command.id).await?.state,
                CommandState::CancelRequested
            );
        }
        let mut result = CommandResult {
            id: command.id,
            status: if cancelling {
                CommandStatus::Cancelled
            } else {
                CommandStatus::Succeeded
            },
            finished_at: now_timestamp(),
            stdout: "fixture".into(),
            stderr: String::new(),
            timed_out: false,
            truncated: false,
        };
        // Completion is allowed to arrive before the durable start outbox.
        for _ in 0..2 {
            panel
                .client
                .post(&endpoint)
                .bearer_auth(&ack.session_token)
                .json(&result)
                .send()
                .await?
                .error_for_status()?;
            panel
                .client
                .post(format!("{endpoint}/started"))
                .bearer_auth(&ack.session_token)
                .json(&started)
                .send()
                .await?
                .error_for_status()?;
        }
        let control: CommandControl = panel
            .client
            .get(format!("{endpoint}/control"))
            .bearer_auth(&ack.session_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(
            control.state,
            if cancelling {
                CommandState::Cancelled
            } else {
                CommandState::Succeeded
            }
        );
        assert_eq!(control.started_at, Some(started.started_at));
        result.stdout = "changed".into();
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
        let changed = CommandStarted {
            started_at: started.started_at + 1,
            ..started
        };
        assert_eq!(
            panel
                .client
                .post(format!("{endpoint}/started"))
                .bearer_auth(&ack.session_token)
                .json(&changed)
                .send()
                .await?
                .status(),
            StatusCode::CONFLICT
        );
    }
    Ok(())
}

#[sqlx::test]
async fn legacy_and_non_cancelling_devices_do_not_promise_running_cancellation(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "legacy commands")
        .await?;
    capabilities(&panel, server, json!(["command:execute"])).await?;
    let command = create(&panel, &cookie, server).await?;
    let pending: Vec<RemoteCommand> = panel
        .client
        .get(format!("{}/api/agent/v1/commands", panel.base))
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(pending, vec![command.clone()]);
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &format!("/api/servers/{server}/commands/{}/cancel", command.id),
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    capabilities(
        &panel,
        server,
        json!(["command:execute", "command:lifecycle:v1"]),
    )
    .await?;
    let command = create(&panel, &cookie, server).await?;
    let claim_id = Uuid::new_v4();
    claim(&panel, &ack.session_token, command.id, claim_id).await?;
    panel
        .client
        .post(format!(
            "{}/api/agent/v1/commands/{}/started",
            panel.base, command.id
        ))
        .bearer_auth(&ack.session_token)
        .json(&CommandStarted {
            claim_id,
            started_at: now_timestamp(),
        })
        .send()
        .await?
        .error_for_status()?;
    // Claim expiry never turns an already running command into "expired".
    sqlx::query("UPDATE remote_commands SET spec=jsonb_set(spec,'{expires_at}',to_jsonb($2::bigint)) WHERE id=$1")
        .bind(command.id).bind(now_timestamp()-1).execute(&panel.state.pool).await?;
    let rows: Vec<Value> = panel
        .admin(
            Method::GET,
            &format!("/api/servers/{server}/commands"),
            &cookie,
            None,
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        rows.iter()
            .find(|row| row["spec"]["id"] == command.id.to_string())
            .unwrap()["state"],
        "running"
    );
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &format!("/api/servers/{server}/commands/{}/cancel", command.id),
                &cookie,
                None
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let queued = create(&panel, &cookie, server).await?;
    assert_eq!(
        cancel(&panel, &cookie, server, queued.id).await?.state,
        CommandState::Cancelled
    );
    Ok(())
}

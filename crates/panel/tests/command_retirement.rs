#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_protocol::{
    CommandClaim, CommandControl, CommandResult, CommandStarted, CommandState, CommandStatus,
    RemoteCommand, TaskAck, now_timestamp,
};
use business_support::TestPanel;
use reqwest::{Method, StatusCode, header};
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

#[sqlx::test]
async fn retirement_serializes_creation_delivery_and_claim_but_accepts_existing_reports(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel
        .authenticated_device(&cookie, "retiring commands")
        .await?;
    sqlx::query("UPDATE servers SET capabilities=$2 WHERE id=$1")
        .bind(server)
        .bind(json!([
            "command:execute",
            "command:lifecycle:v1",
            "command:cancel:v1"
        ]))
        .execute(&panel.state.pool)
        .await?;
    let body = json!({"command":"printf fixture", "timeout_secs":5, "ttl_secs":60});
    let mut commands = Vec::new();
    for _ in 0..3 {
        commands.push(
            panel
                .admin(
                    Method::POST,
                    &format!("/api/servers/{server}/commands"),
                    &cookie,
                    Some(body.clone()),
                )
                .await?
                .error_for_status()?
                .json::<RemoteCommand>()
                .await?,
        );
    }
    let active = &commands[0];
    let queued = &commands[1];
    sqlx::query(
        "UPDATE remote_commands SET lifecycle_version=0,cancel_supported=FALSE WHERE id=$1",
    )
    .bind(commands[2].id)
    .execute(&panel.state.pool)
    .await?;
    let endpoint = format!("{}/api/agent/v1/commands", panel.base);
    let claim_id = Uuid::new_v4();
    let control: CommandControl = panel
        .client
        .post(format!("{endpoint}/{}/claim", active.id))
        .bearer_auth(&ack.session_token)
        .json(&CommandClaim { claim_id })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(control.state, CommandState::Claimed);

    let mut retiring = panel.state.pool.begin().await?;
    sqlx::query("SELECT id FROM servers WHERE id=$1 FOR UPDATE")
        .bind(server)
        .fetch_one(&mut *retiring)
        .await?;
    let create = panel
        .client
        .post(format!("{}/api/servers/{server}/commands", panel.base))
        .header(header::COOKIE, &cookie)
        .json(&body);
    let claim = panel
        .client
        .post(format!("{endpoint}/{}/claim", queued.id))
        .bearer_auth(&ack.session_token)
        .json(&CommandClaim {
            claim_id: Uuid::new_v4(),
        });
    let legacy = panel.client.get(&endpoint).bearer_auth(&ack.session_token);
    let lifecycle = panel
        .client
        .get(format!("{endpoint}/lifecycle"))
        .bearer_auth(&ack.session_token);
    let requests =
        async { tokio::try_join!(create.send(), claim.send(), legacy.send(), lifecycle.send()) };
    tokio::pin!(requests);
    assert!(
        tokio::time::timeout(Duration::from_millis(150), &mut requests)
            .await
            .is_err()
    );
    sqlx::query("INSERT INTO server_retirements(server_id,request_id,status,requested_at) VALUES($1,$2,'pending',$3)")
        .bind(server).bind(Uuid::new_v4()).bind(now_timestamp()).execute(&mut *retiring).await?;
    retiring.commit().await?;
    let (created, claimed, legacy, lifecycle) = requests.await?;
    assert_eq!(created.status(), StatusCode::CONFLICT);
    assert_eq!(claimed.status(), StatusCode::CONFLICT);
    assert!(
        legacy
            .error_for_status()?
            .json::<Vec<RemoteCommand>>()
            .await?
            .is_empty()
    );
    assert!(
        lifecycle
            .error_for_status()?
            .json::<Vec<RemoteCommand>>()
            .await?
            .is_empty()
    );
    let queued_states: Vec<String> =
        sqlx::query_scalar("SELECT state FROM remote_commands WHERE id=ANY($1)")
            .bind(vec![queued.id, commands[2].id])
            .fetch_all(&panel.state.pool)
            .await?;
    assert_eq!(queued_states, ["queued", "queued"]);

    // Existing execution remains reportable while retirement is pending. A late
    // start report and immutable result must not be stranded by the start gate.
    let started_at = now_timestamp().max(control.claimed_at.unwrap());
    panel
        .client
        .post(format!("{endpoint}/{}/started", active.id))
        .bearer_auth(&ack.session_token)
        .json(&CommandStarted {
            claim_id,
            started_at,
        })
        .send()
        .await?
        .error_for_status()?;
    let result = CommandResult {
        id: active.id,
        status: CommandStatus::Interrupted,
        finished_at: started_at,
        stdout: String::new(),
        stderr: "fixture retirement cleanup".into(),
        timed_out: false,
        truncated: false,
    };
    for _ in 0..2 {
        let received: TaskAck = panel
            .client
            .post(format!("{endpoint}/{}", active.id))
            .bearer_auth(&ack.session_token)
            .json(&result)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(received.ids, [active.id]);
    }
    let control: CommandControl = panel
        .client
        .get(format!("{endpoint}/{}/control", active.id))
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(control.state, CommandState::Interrupted);
    assert_eq!(control.started_at, Some(started_at));
    Ok(())
}

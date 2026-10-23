#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
use anyhow::Result;
use apsp_protocol::{
    RuntimeValidationError, RuntimeValidationOperation, RuntimeValidationRequest,
    RuntimeValidationResult, TaskAck, now_timestamp,
};
use business_support::TestPanel;
use reqwest::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;

async fn insert(pool: &PgPool, server: i64, request: &RuntimeValidationRequest) -> Result<()> {
    sqlx::query("INSERT INTO runtime_validations(id,server_id,module,scope,generation,operation,revision,config_hash,expires_at,requested_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(request.id).bind(server).bind(&request.module).bind(&request.scope).bind(request.generation as i64).bind(match request.operation { RuntimeValidationOperation::Probe=>"probe",RuntimeValidationOperation::Barrier=>"barrier" }).bind(request.revision as i64).bind(&request.config_hash).bind(request.expires_at).bind(now_timestamp()).execute(pool).await?;
    Ok(())
}

#[sqlx::test]
async fn validation_queue_is_device_scoped_and_binds_every_proof_field(pool: PgPool) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "validation").await?;
    let (_, _other_socket, other_ack) = panel.authenticated_device(&cookie, "other").await?;
    let request = RuntimeValidationRequest {
        id: Uuid::new_v4(),
        module: "demo".into(),
        scope: "scope:1".into(),
        generation: 2,
        operation: RuntimeValidationOperation::Probe,
        revision: 3,
        config_hash: "a".repeat(64),
        expires_at: now_timestamp() + 60,
    };
    insert(&panel.state.pool, server, &request).await?;
    let endpoint = format!("{}/api/agent/v1/runtime-validations", panel.base);
    assert_eq!(
        panel.client.get(&endpoint).send().await?.status(),
        StatusCode::UNAUTHORIZED
    );
    let other: Vec<RuntimeValidationRequest> = panel
        .client
        .get(&endpoint)
        .bearer_auth(&other_ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(other.is_empty());
    let pending: Vec<RuntimeValidationRequest> = panel
        .client
        .get(&endpoint)
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(pending.as_slice(), std::slice::from_ref(&request));
    let endpoint = format!("{endpoint}/{}/result", request.id);
    let result = RuntimeValidationResult {
        request: request.clone(),
        success: true,
        error: None,
        checked_at: now_timestamp(),
    };
    assert_eq!(
        panel
            .client
            .post(&endpoint)
            .bearer_auth(&other_ack.session_token)
            .json(&result)
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    for mutate in 0..4 {
        let mut invalid = result.clone();
        match mutate {
            0 => invalid.request.config_hash = "b".repeat(64),
            1 => invalid.request.generation += 1,
            2 => invalid.request.revision += 1,
            _ => invalid.request.scope = "scope:2".into(),
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
            StatusCode::CONFLICT
        );
    }
    for _ in 0..2 {
        let ack: TaskAck = panel
            .client
            .post(&endpoint)
            .bearer_auth(&ack.session_token)
            .json(&result)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(ack.ids, [request.id]);
    }
    let changed = RuntimeValidationResult {
        success: false,
        error: Some(RuntimeValidationError::ValidationFailed),
        ..result
    };
    assert_eq!(
        panel
            .client
            .post(&endpoint)
            .bearer_auth(&ack.session_token)
            .json(&changed)
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    Ok(())
}

#[sqlx::test]
async fn expired_barrier_attempt_is_not_delivered_and_new_attempt_keeps_original_immutable(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "barrier").await?;
    let expired = RuntimeValidationRequest {
        id: Uuid::new_v4(),
        module: "demo".into(),
        scope: "scope:1".into(),
        generation: 2,
        operation: RuntimeValidationOperation::Barrier,
        revision: 3,
        config_hash: "a".repeat(64),
        expires_at: now_timestamp() - 1,
    };
    insert(&panel.state.pool, server, &expired).await?;
    let retry = RuntimeValidationRequest {
        id: Uuid::new_v4(),
        expires_at: now_timestamp() + 60,
        ..expired.clone()
    };
    insert(&panel.state.pool, server, &retry).await?;
    let endpoint = format!("{}/api/agent/v1/runtime-validations", panel.base);
    let pending: Vec<RuntimeValidationRequest> = panel
        .client
        .get(&endpoint)
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(pending, [retry]);
    let late = RuntimeValidationResult {
        request: expired.clone(),
        success: true,
        error: None,
        checked_at: now_timestamp(),
    };
    assert_eq!(
        panel
            .client
            .post(format!("{endpoint}/{}/result", expired.id))
            .bearer_auth(&ack.session_token)
            .json(&late)
            .send()
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    let recorded: i64 =
        sqlx::query_scalar("SELECT expires_at FROM runtime_validations WHERE id=$1")
            .bind(expired.id)
            .fetch_one(&panel.state.pool)
            .await?;
    assert_eq!(recorded, expired.expires_at);
    Ok(())
}

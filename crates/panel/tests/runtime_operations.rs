#![forbid(unsafe_code)]

mod business_support;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
use anyhow::Result;
use apsp_protocol::{
    RuntimeOperationError, RuntimeOperationRequest, RuntimeOperationResult, TaskAck, now_timestamp,
};
use business_support::TestPanel;
use reqwest::{Method, StatusCode};
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test]
async fn runtime_operations_are_capability_gated_device_scoped_and_immutable(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, ack) = panel.authenticated_device(&cookie, "runtime").await?;
    let (_, _other_socket, other_ack) = panel.authenticated_device(&cookie, "other").await?;
    let path = format!("/api/plugins/sing-box/servers/{server}/runtime-operations");
    sqlx::query("UPDATE servers SET capabilities='[\"singbox\"]' WHERE id=$1")
        .bind(server)
        .execute(&panel.state.pool)
        .await?;
    let inspect = json!({"operation":"inspect","expected_revision":null});
    assert_eq!(
        panel
            .client
            .post(format!("{}{path}", panel.base))
            .json(&inspect)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let disabled = panel
        .admin(Method::POST, &path, &cookie, Some(inspect.clone()))
        .await?;
    assert_eq!(disabled.status(), StatusCode::CONFLICT);
    assert!(
        disabled.json::<serde_json::Value>().await?["error"]
            .as_str()
            .unwrap()
            .contains("尚未启用 sing-box")
    );
    // Capability advertisements do not authorize plugin enablement.
    panel.enable_plugin(&cookie, server).await?;
    let unsupported = panel
        .admin(Method::POST, &path, &cookie, Some(inspect.clone()))
        .await?;
    assert_eq!(unsupported.status(), StatusCode::CONFLICT);
    assert!(
        unsupported.json::<serde_json::Value>().await?["error"]
            .as_str()
            .unwrap()
            .contains("Agent 尚不支持运行时运维")
    );
    sqlx::query("UPDATE servers SET capabilities=capabilities || '[\"runtime:operations:v1\"]'::jsonb WHERE id=$1").bind(server).execute(&panel.state.pool).await?;
    let request: RuntimeOperationRequest = panel
        .admin(Method::POST, &path, &cookie, Some(inspect.clone()))
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        panel
            .admin(Method::POST, &path, &cookie, Some(inspect))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let pending_url = format!("{}/api/agent/v1/runtime-operations", panel.base);
    let other: Vec<RuntimeOperationRequest> = panel
        .client
        .get(&pending_url)
        .bearer_auth(&other_ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(other.is_empty());
    let pending: Vec<RuntimeOperationRequest> = panel
        .client
        .get(&pending_url)
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(pending.as_slice(), std::slice::from_ref(&request));
    let result = RuntimeOperationResult {
        id: request.id,
        module: request.module.clone(),
        operation: request.operation,
        finished_at: now_timestamp(),
        error: Some(RuntimeOperationError::Interrupted),
        snapshot: None,
    };
    let complete = format!("{pending_url}/{}", request.id);
    assert_eq!(
        panel
            .client
            .post(&complete)
            .bearer_auth(&other_ack.session_token)
            .json(&result)
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    for _ in 0..2 {
        let response: TaskAck = panel
            .client
            .post(&complete)
            .bearer_auth(&ack.session_token)
            .json(&result)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(response.ids, [request.id]);
    }
    let changed = RuntimeOperationResult {
        error: Some(RuntimeOperationError::OperationFailed),
        ..result
    };
    assert_eq!(
        panel
            .client
            .post(&complete)
            .bearer_auth(&ack.session_token)
            .json(&changed)
            .send()
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    let pending: Vec<RuntimeOperationRequest> = panel
        .client
        .get(&pending_url)
        .bearer_auth(&ack.session_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(pending.is_empty());
    Ok(())
}

#[sqlx::test]
async fn runtime_mutations_require_current_failed_target_and_refuse_retirement(
    pool: PgPool,
) -> Result<()> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let (server, _socket, _) = panel.authenticated_device(&cookie, "target").await?;
    let path = format!("/api/plugins/sing-box/servers/{server}/runtime-operations");
    let disabled = panel
        .admin(
            Method::POST,
            &path,
            &cookie,
            Some(json!({"operation":"inspect","expected_revision":null})),
        )
        .await?;
    assert_eq!(disabled.status(), StatusCode::CONFLICT);
    assert!(
        disabled.json::<serde_json::Value>().await?["error"]
            .as_str()
            .unwrap()
            .contains("尚未启用 sing-box")
    );
    panel.enable_plugin(&cookie, server).await?;
    sqlx::query("UPDATE servers SET capabilities='[\"singbox\",\"runtime:operations:v1\"]',dirty_at=NULL WHERE id=$1").bind(server).execute(&panel.state.pool).await?;
    sqlx::query("INSERT INTO server_module_status(server_id,module,target_rev,applied_rev,last_result_rev,healthy,last_error,updated_at) VALUES($1,'singbox',2,1,2,true,'fixture failure',$2)").bind(server).bind(now_timestamp()).execute(&panel.state.pool).await?;
    for body in [
        json!({"operation":"retry_deployment","expected_revision":1}),
        json!({"operation":"restart","expected_revision":2}),
    ] {
        assert_eq!(
            panel
                .admin(Method::POST, &path, &cookie, Some(body))
                .await?
                .status(),
            StatusCode::CONFLICT
        );
    }
    sqlx::query("UPDATE servers SET dirty_at=1 WHERE id=$1")
        .bind(server)
        .execute(&panel.state.pool)
        .await?;
    let retry = json!({"operation":"retry_deployment","expected_revision":2});
    assert_eq!(
        panel
            .admin(Method::POST, &path, &cookie, Some(retry.clone()))
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    sqlx::query("UPDATE servers SET dirty_at=NULL WHERE id=$1")
        .bind(server)
        .execute(&panel.state.pool)
        .await?;
    assert_eq!(
        panel
            .admin(Method::POST, &path, &cookie, Some(retry))
            .await?
            .status(),
        StatusCode::OK
    );
    // Retirement prevents any further delivery even for an already queued request.
    sqlx::query("INSERT INTO server_retirements(server_id,request_id,status,requested_at) VALUES($1,$2,'pending',$3)").bind(server).bind(uuid::Uuid::new_v4()).bind(now_timestamp()).execute(&panel.state.pool).await?;
    assert_eq!(
        panel
            .admin(
                Method::POST,
                &path,
                &cookie,
                Some(json!({"operation":"inspect","expected_revision":null}))
            )
            .await?
            .status(),
        StatusCode::CONFLICT
    );
    Ok(())
}

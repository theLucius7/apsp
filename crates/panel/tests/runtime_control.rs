#![forbid(unsafe_code)]

#[path = "runtime_control/api.rs"]
mod api;
#[path = "runtime_control/barriers.rs"]
mod barriers;
mod business_support;
#[path = "runtime_control/exact.rs"]
mod exact;
#[path = "runtime_control/path_probe.rs"]
mod path_probe;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::Result;
use apsp_panel::{AgentConnection, AppState, agent_api, runtime_control};
use apsp_protocol::*;
use business_support::TestPanel;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::BTreeMap;
use tokio::sync::mpsc;
use uuid::Uuid;

const MODULE: &str = "test-runtime";

async fn fixture(pool: PgPool) -> Result<(TestPanel, i64)> {
    let panel = TestPanel::start(pool).await?;
    let cookie = panel.admin_cookie().await?;
    let server = panel.create_server(&cookie, "运行配置夹具").await?;
    publish(&panel.state.pool, server, 1).await?;
    introduce(&panel.state, server, true, BTreeMap::new()).await?;
    Ok((panel, server))
}

async fn publish(pool: &PgPool, server: i64, revision: i64) -> Result<()> {
    let hash = if revision == 1 { "a" } else { "b" }.repeat(64);
    sqlx::query("INSERT INTO deployments(server_id,module,rev,bundle,bundle_sha256,created_at) VALUES($1,$2,$3,'TEST_ONLY configuration',$4,$5)")
        .bind(server).bind(MODULE).bind(revision).bind(hash).bind(now_timestamp()).execute(pool).await?;
    sqlx::query("INSERT INTO server_module_status(server_id,module,target_rev) VALUES($1,$2,$3) ON CONFLICT(server_id,module) DO UPDATE SET target_rev=EXCLUDED.target_rev,healthy=false")
        .bind(server).bind(MODULE).bind(revision).execute(pool).await?;
    sqlx::query("UPDATE servers SET dirty_at=NULL WHERE id=$1")
        .bind(server)
        .execute(pool)
        .await?;
    Ok(())
}

async fn introduce(
    state: &AppState,
    server: i64,
    capable: bool,
    applied: AppliedRevisions,
) -> Result<()> {
    agent_api::process_message(
        state,
        server,
        Message::Hello(Hello {
            agent_version: "TEST_ONLY".into(),
            protocol_version: PROTOCOL_VERSION,
            capabilities: if capable {
                vec![
                    RUNTIME_CHECKPOINT_CAPABILITY.into(),
                    RUNTIME_RECOVERY_BARRIER_CAPABILITY.into(),
                ]
            } else {
                Vec::new()
            },
            applied,
        }),
    )
    .await
}

fn applied(revision: u64) -> ApplyResult {
    ApplyResult {
        module: MODULE.into(),
        rev: revision,
        op_id: Uuid::new_v4(),
        status: ApplyStatus::Applied,
        healthy: true,
        error: None,
    }
}

fn success(request: &RuntimeCheckpointRequest) -> RuntimeCheckpointResult {
    RuntimeCheckpointResult {
        request_id: request.request_id,
        request_digest: request.digest().unwrap(),
        observed: Some(RuntimeCheckpoint {
            binding: request.expected.clone(),
            activation_id: Uuid::from_u128(100),
            instance_id: "backend:TEST_ONLY-instance".into(),
            healthy: true,
        }),
        success: true,
        error: None,
    }
}

fn barrier_success(request: &RuntimeRecoveryBarrierRequest) -> RuntimeRecoveryBarrierResult {
    RuntimeRecoveryBarrierResult {
        request_id: request.request_id,
        request_digest: request.digest().unwrap(),
        observed: Some(request.expected.clone()),
        minimum_revision: Some(request.minimum_revision),
        pending_intents_clear: true,
        success: true,
        error: None,
    }
}

async fn pending(pool: &PgPool, server: i64) -> Result<RuntimeCheckpointRequest> {
    let payload: Value = sqlx::query_scalar("SELECT request_json FROM runtime_control_requests WHERE server_id=$1 AND kind='checkpoint' AND state='pending'")
        .bind(server).fetch_one(pool).await?;
    Ok(serde_json::from_value(payload)?)
}

async fn health(pool: &PgPool, server: i64) -> Result<(i64, bool)> {
    Ok(sqlx::query_as(
        "SELECT applied_rev,healthy FROM server_module_status WHERE server_id=$1 AND module=$2",
    )
    .bind(server)
    .bind(MODULE)
    .fetch_one(pool)
    .await?)
}

async fn outcome(pool: &PgPool, request_id: Uuid) -> Result<String> {
    Ok(
        sqlx::query_scalar("SELECT outcome FROM runtime_control_receipts WHERE request_id=$1")
            .bind(request_id)
            .fetch_one(pool)
            .await?,
    )
}

async fn confirmed(state: &AppState, server: i64) -> Result<RuntimeCheckpoint> {
    let request = runtime_control::request_checkpoint(state, server, MODULE).await?;
    let result = success(&request);
    let observed = result.observed.clone().unwrap();
    runtime_control::record_checkpoint_result(state, server, result).await?;
    Ok(observed)
}

async fn attach(state: &AppState, server: i64) -> mpsc::Receiver<Envelope> {
    let (sender, receiver) = mpsc::channel(64);
    state.connections.write().await.insert(
        server,
        AgentConnection {
            id: Uuid::new_v4(),
            sender,
        },
    );
    receiver
}

async fn receive(receiver: &mut mpsc::Receiver<Envelope>) -> Result<Message> {
    let envelope = tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv())
        .await?
        .ok_or_else(|| anyhow::anyhow!("runtime notification channel closed"))?;
    Ok(envelope.decode()?)
}

async fn count(pool: &PgPool, table: &str) -> Result<i64> {
    let query = format!("SELECT count(*) FROM {table}");
    Ok(sqlx::query_scalar(&query).fetch_one(pool).await?)
}

async fn expire(pool: &PgPool, request: &mut RuntimeCheckpointRequest) -> Result<()> {
    request.expires_at = now_timestamp() - 1;
    sqlx::query("UPDATE runtime_control_requests SET created_at=$2-120,expires_at=$2,request_json=$3,request_digest=$4 WHERE request_id=$1")
        .bind(request.request_id).bind(request.expires_at).bind(json!(request)).bind(request.digest()?).execute(pool).await?;
    Ok(())
}

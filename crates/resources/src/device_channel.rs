//! The device channel: authenticated WebSocket transport, device sessions and presence.
//!
//! A device proves the key bound to its server, gets a session for its HTTP requests
//! and stays connected. Reports are handed to the domains that own them through
//! [`DeviceReports`], which the application connects; the channel itself keeps only the
//! protocol.

use crate::{
    AppState,
    error::{ApiError, ApiResult},
    ports::{BoxFuture, Port},
    tokens,
};
use apsp_protocol::{
    AppliedRevisions, ApplyResult, AuthChallenge, DiagnosticCancelResult, Envelope, HelloAck,
    Message, Metrics, RetirementResult, RuntimeCheckpointResult, RuntimeControlAck,
    RuntimePathProbeResult, RuntimeRecoveryBarrierResult, RuntimeSessionReport, StaticInfo,
    UsageBatch, now_timestamp,
};
use axum::{
    Router,
    extract::{
        State,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, header},
    response::Response,
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, VerifyingKey};
use futures_util::{SinkExt, StreamExt};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex, RwLock, mpsc},
    time::{Instant, timeout},
};
use uuid::Uuid;

/// One device's live connection: the sender that reaches its WebSocket.
#[derive(Clone)]
pub struct AgentConnection {
    pub id: Uuid,
    pub sender: mpsc::Sender<Envelope>,
}

/// The devices connected to this process. `lifecycle` orders a connection's registration
/// and removal against the retirement of its server.
#[derive(Default)]
pub struct Devices {
    pub connections: RwLock<HashMap<i64, AgentConnection>>,
    pub lifecycle: Mutex<()>,
}

/// The connected devices, shared by every copy of the application state.
pub fn devices(state: &AppState) -> Arc<Devices> {
    state.domain_states.get_or_init(Devices::default)
}

/// The server a device request belongs to, from the session the channel issued.
pub async fn require_agent(state: &AppState, headers: &HeaderMap) -> ApiResult<i64> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty() && value.len() <= 512)
        .ok_or(ApiError::Unauthorized)?;
    sqlx::query_scalar::<_, i64>("SELECT d.server_id FROM device_sessions d JOIN servers ON servers.id = d.server_id WHERE d.token_hash = $1 AND d.expires_at > $2 AND servers.deleted_at IS NULL")
        .bind(tokens::hash_token(token)).bind(now_timestamp()).fetch_optional(&state.pool).await?.ok_or(ApiError::Unauthorized)
}

/// A device session lasts an hour and is reissued on every reconnect; expired ones can never
/// authenticate again and are removed by the maintenance cycle.
pub async fn purge_expired_sessions(pool: &sqlx::PgPool, now: i64) -> anyhow::Result<u64> {
    Ok(
        sqlx::query("DELETE FROM device_sessions WHERE expires_at <= $1")
            .bind(now)
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

/// Refuses signed Agent update offers for historical Agents that cannot verify release signatures.
pub async fn require_signed_agent(state: &AppState, server_id: i64) -> ApiResult<()> {
    let capabilities: serde_json::Value =
        sqlx::query_scalar("SELECT capabilities FROM servers WHERE id=$1 AND deleted_at IS NULL")
            .bind(server_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(ApiError::Unauthorized)?;
    if !capabilities.as_array().is_some_and(|values| {
        values.iter().any(|v| {
            matches!(
                v.as_str(),
                Some(
                    apsp_protocol::SELF_CONTAINED_AGENT_CAPABILITY
                        | apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY
                )
            )
        })
    }) {
        return Err(ApiError::Conflict(
            "此 Agent 尚不支持签名发布升级，请先使用可信安装器升级；已运行配置和流量上报继续保留"
                .into(),
        ));
    }
    Ok(())
}

/// Reports of an authenticated device, answered by the domains that own them.
pub trait DeviceReports: Send + Sync {
    /// Capabilities and applied revisions announced when the device connects.
    fn hello<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        capabilities: Vec<String>,
        applied: AppliedRevisions,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn heartbeat<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        applied: AppliedRevisions,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn static_info<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        info: StaticInfo,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn metrics<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        metrics: Metrics,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn apply_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: ApplyResult,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn checkpoint_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: RuntimeCheckpointResult,
    ) -> BoxFuture<'a, anyhow::Result<RuntimeControlAck>>;
    fn path_probe_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: RuntimePathProbeResult,
    ) -> BoxFuture<'a, anyhow::Result<RuntimeControlAck>>;
    fn barrier_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: RuntimeRecoveryBarrierResult,
    ) -> BoxFuture<'a, anyhow::Result<RuntimeControlAck>>;
    fn diagnostic_cancel_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: DiagnosticCancelResult,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    /// Records one usage batch; an error leaves the batch unacknowledged.
    fn usage_batch<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        batch: UsageBatch,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    /// Admitted runtime session addresses; devices report them only when session
    /// control is enabled in their configuration.
    fn runtime_sessions<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        report: RuntimeSessionReport,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn retirement_result<'a>(
        &'a self,
        state: &'a AppState,
        server_id: i64,
        result: RetirementResult,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
}

static REPORTS: Port<dyn DeviceReports> = Port::new();

/// Connects the device reports to their owners; later calls keep the first one.
pub fn install(reports: &'static dyn DeviceReports) {
    REPORTS.install(reports);
}

fn reports() -> anyhow::Result<&'static dyn DeviceReports> {
    REPORTS
        .get()
        .ok_or_else(|| anyhow::anyhow!("device reports are not connected"))
}

/// The device channel's own route; every other device endpoint belongs to a domain.
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/agent/v1/ws", get(websocket))
}

pub async fn websocket(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(1024 * 1024)
        .on_upgrade(move |socket| async move {
            if let Err(error) = connection(socket, state).await {
                tracing::debug!(%error, "device connection ended");
            }
        })
}

async fn connection(mut socket: WebSocket, state: AppState) -> anyhow::Result<()> {
    let nonce = tokens::random_token();
    send(
        &mut socket,
        Envelope::new(
            "auth.challenge",
            AuthChallenge {
                nonce: nonce.clone(),
                server_time: now_timestamp(),
            },
        )?,
    )
    .await?;
    let text = timeout(Duration::from_secs(10), socket.recv())
        .await?
        .ok_or_else(|| anyhow::anyhow!("closed during authentication"))??;
    let WsMessage::Text(text) = text else {
        anyhow::bail!("expected authentication response");
    };
    let envelope: Envelope = serde_json::from_str(&text)?;
    anyhow::ensure!(envelope.v == 1, "unsupported protocol version");
    let Message::AuthResponse(response) = envelope.decode()? else {
        anyhow::bail!("expected authentication response");
    };
    let public_key: Option<String> = sqlx::query_scalar(
        "SELECT device_public_key FROM servers WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(response.server_id)
    .fetch_optional(&state.pool)
    .await?
    .flatten();
    let public_key = public_key.ok_or_else(|| anyhow::anyhow!("unknown device"))?;
    let bytes: [u8; 32] = URL_SAFE_NO_PAD
        .decode(&public_key)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid public key"))?;
    let signature = Signature::from_slice(&URL_SAFE_NO_PAD.decode(response.signature)?)?;
    VerifyingKey::from_bytes(&bytes)?.verify_strict(nonce.as_bytes(), &signature)?;
    let server_id = response.server_id;
    let session_token = tokens::random_token();
    let server_time = now_timestamp();
    let expires_at = server_time + 3600;
    let (sender, mut receiver) = mpsc::channel::<Envelope>(32);
    let connection_id = Uuid::new_v4();
    register_connection(
        &state,
        server_id,
        &public_key,
        &session_token,
        expires_at,
        AgentConnection {
            id: connection_id,
            sender,
        },
    )
    .await?;
    let devices = devices(&state);
    let result: anyhow::Result<()> = async {
        send(
            &mut socket,
            Envelope::new(
                "hello.ack",
                HelloAck {
                    server_time,
                    session_token,
                    session_expires_at: expires_at,
                },
            )?,
        )
        .await?;
        let (mut sink, mut stream) = socket.split();
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        let mut last_received = Instant::now();
        let mut introduced = false;
        loop {
            tokio::select! {
                frame = stream.next() => {
                    let Some(frame) = frame else { break; };
                    let frame = frame?;
                    if !is_current_connection(&devices, server_id, connection_id).await {
                        break;
                    }
                    last_received = Instant::now();
                    match frame {
                        WsMessage::Text(text) => {
                            let envelope: Envelope = serde_json::from_str(&text)?;
                            anyhow::ensure!(envelope.v == 1, "unsupported protocol version");
                            let message = envelope.decode()?;
                            if !introduced {
                                let Message::Hello(hello) = &message else { anyhow::bail!("expected hello"); };
                                anyhow::ensure!((apsp_protocol::PROTOCOL_MIN..=apsp_protocol::PROTOCOL_MAX).contains(&hello.protocol_version), "unsupported protocol version");
                                introduced = true;
                            }
                            if !process_connection_message(&state, server_id, connection_id, message).await? {
                                break;
                            }
                        }
                        WsMessage::Ping(bytes) => sink.send(WsMessage::Pong(bytes)).await?,
                        WsMessage::Close(_) => break,
                        WsMessage::Pong(_) => {},
                        _ => anyhow::bail!("expected text message"),
                    }
                    if !is_current_connection(&devices, server_id, connection_id).await {
                        break;
                    }
                    sqlx::query("UPDATE servers SET last_seen=$2,last_contact_at=$2 WHERE id=$1 AND deleted_at IS NULL")
                        .bind(server_id)
                        .bind(now_timestamp())
                        .execute(&state.pool)
                        .await?;
                }
                notification = receiver.recv() => {
                    let Some(notification) = notification else { break; };
                    sink.send(WsMessage::Text(serde_json::to_string(&notification)?.into())).await?;
                }
                _ = heartbeat.tick() => {
                    if !is_current_connection(&devices, server_id, connection_id).await
                        || last_received.elapsed() > Duration::from_secs(60)
                        || now_timestamp() >= expires_at
                    {
                        break;
                    }
                    sink.send(WsMessage::Ping(Vec::new().into())).await?;
                }
            }
        }
        Ok(())
    }
    .await;
    unregister_connection(&state, server_id, connection_id).await?;
    result
}

async fn is_current_connection(devices: &Devices, server_id: i64, connection_id: Uuid) -> bool {
    devices
        .connections
        .read()
        .await
        .get(&server_id)
        .is_some_and(|entry| entry.id == connection_id)
}

async fn register_connection(
    state: &AppState,
    server_id: i64,
    public_key: &str,
    session_token: &str,
    expires_at: i64,
    connection: AgentConnection,
) -> anyhow::Result<()> {
    let devices = devices(state);
    let _lifecycle = devices.lifecycle.lock().await;
    let mut tx = state.pool.begin().await?;
    let current: Option<String> = crate::servers::lock_row(
        &mut tx,
        server_id,
        crate::servers::RowLock::Update,
        "device_public_key",
    )
    .await?
    .and_then(|row| sqlx::Row::get(&row, "device_public_key"));
    anyhow::ensure!(
        current.as_deref() == Some(public_key),
        "device was deleted or rebound during authentication"
    );
    sqlx::query("INSERT INTO device_sessions(token_hash,server_id,expires_at) VALUES($1,$2,$3)")
        .bind(tokens::hash_token(session_token))
        .bind(server_id)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
    // Clear the previous connection's static_info freshness eligibility while leaving
    // historical static_info read-only until this connection reports its own identity.
    sqlx::query(
        "UPDATE servers SET last_seen=$2,last_contact_at=$2,static_info_received_at=NULL WHERE id=$1",
    )
    .bind(server_id)
    .bind(now_timestamp())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    devices
        .connections
        .write()
        .await
        .insert(server_id, connection);
    Ok(())
}

async fn record_connection_static_info(
    state: &AppState,
    server_id: i64,
    connection_id: Uuid,
    info: StaticInfo,
) -> anyhow::Result<bool> {
    let devices = devices(state);
    let _lifecycle = devices.lifecycle.lock().await;
    // Check the leaf connection map before touching the database; holding `lifecycle`
    // orders this static identity write against any newer connection's registration
    // and against server retirement.
    if !is_current_connection(&devices, server_id, connection_id).await {
        return Ok(false);
    }
    reports()?.static_info(state, server_id, info).await?;
    Ok(true)
}

async fn process_connection_message(
    state: &AppState,
    server_id: i64,
    connection_id: Uuid,
    message: Message,
) -> anyhow::Result<bool> {
    let devices = devices(state);
    if !is_current_connection(&devices, server_id, connection_id).await {
        return Ok(false);
    }
    match message {
        Message::TelemetryStatic(info) => {
            return record_connection_static_info(state, server_id, connection_id, info).await;
        }
        // The device keeps a rejected batch unacknowledged in its durable
        // outbox and replays it. Tearing down the channel here would let one
        // invalid batch block later batches and every other control message.
        Message::UsageBatch(batch) => {
            if let Err(error) = usage_batch(state, server_id, batch).await {
                tracing::warn!(
                    server_id,
                    %error,
                    "usage batch rejected and left unacknowledged"
                );
            }
        }
        message => process_message(state, server_id, message).await?,
    }
    Ok(is_current_connection(&devices, server_id, connection_id).await)
}

async fn unregister_connection(
    state: &AppState,
    server_id: i64,
    connection_id: Uuid,
) -> anyhow::Result<()> {
    let devices = devices(state);
    let _lifecycle = devices.lifecycle.lock().await;
    // Release the connection map before touching the database: every agent's
    // notifications read it. The lifecycle mutex still orders this update
    // against a reconnect, which registers under the same mutex.
    let removed = {
        let mut connections = devices.connections.write().await;
        let current = connections
            .get(&server_id)
            .is_some_and(|entry| entry.id == connection_id);
        if current {
            connections.remove(&server_id);
        }
        current
    };
    if removed {
        // Leave the online window at once; offline alerts measure from the real contact time.
        let now = now_timestamp();
        sqlx::query("UPDATE servers SET last_seen=$2,last_contact_at=$3 WHERE id=$1")
            .bind(server_id)
            .bind(now - 61)
            .bind(now)
            .execute(&state.pool)
            .await?;
    }
    Ok(())
}

async fn send(socket: &mut WebSocket, envelope: Envelope) -> anyhow::Result<()> {
    socket
        .send(WsMessage::Text(serde_json::to_string(&envelope)?.into()))
        .await?;
    Ok(())
}

pub async fn notify(state: &AppState, server_id: i64, envelope: Envelope) {
    let sender = devices(state)
        .connections
        .read()
        .await
        .get(&server_id)
        .map(|entry| entry.sender.clone());
    if let Some(sender) = sender {
        let _ = sender.try_send(envelope);
    }
}

async fn usage_batch(state: &AppState, server_id: i64, batch: UsageBatch) -> anyhow::Result<()> {
    match REPORTS.get() {
        Some(reports) => reports.usage_batch(state, server_id, batch).await,
        None => anyhow::bail!("no domain accepts usage batches"),
    }
}

pub async fn process_message(
    state: &AppState,
    server_id: i64,
    message: Message,
) -> anyhow::Result<()> {
    match message {
        Message::Hello(hello) => {
            let capabilities: Vec<_> = hello
                .capabilities
                .into_iter()
                .filter(|value| value.len() <= 128)
                .take(64)
                .collect();
            reports()?
                .hello(state, server_id, capabilities, hello.applied)
                .await?;
        }
        Message::Heartbeat(heartbeat) => {
            reports()?
                .heartbeat(state, server_id, heartbeat.applied)
                .await?;
        }
        Message::TelemetryStatic(info) => reports()?.static_info(state, server_id, info).await?,
        Message::TelemetryMetrics(metrics) => reports()?.metrics(state, server_id, metrics).await?,
        Message::DiagnosticCancelResult(result) => {
            reports()?
                .diagnostic_cancel_result(state, server_id, result)
                .await?;
        }
        Message::ApplyResult(result) => reports()?.apply_result(state, server_id, result).await?,
        Message::RuntimeCheckpointResult(result) => {
            let ack = reports()?
                .checkpoint_result(state, server_id, result)
                .await?;
            notify(
                state,
                server_id,
                Envelope::new("runtime.checkpoint.ack", ack)?,
            )
            .await;
        }
        Message::RuntimePathProbeResult(result) => {
            let ack = reports()?
                .path_probe_result(state, server_id, result)
                .await?;
            notify(
                state,
                server_id,
                Envelope::new("runtime.path_probe.ack", ack)?,
            )
            .await;
        }
        Message::RuntimeRecoveryBarrierResult(result) => {
            let ack = reports()?.barrier_result(state, server_id, result).await?;
            notify(state, server_id, Envelope::new("runtime.barrier.ack", ack)?).await;
        }
        Message::UsageBatch(batch) => usage_batch(state, server_id, batch).await?,
        Message::RuntimeSessionReport(report) => {
            anyhow::ensure!(report.valid(), "invalid runtime session report");
            if let Some(reports) = REPORTS.get() {
                reports.runtime_sessions(state, server_id, report).await?;
            }
        }
        Message::RetirementResult(result) => {
            reports()?
                .retirement_result(state, server_id, result)
                .await?
        }
        Message::Unknown { message_type, .. } => {
            tracing::debug!(%message_type,"ignoring unknown device message")
        }
        _ => tracing::debug!("ignoring unsupported device message direction"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        servers::{SERVER_COLUMNS, Server},
    };
    use apsp_protocol::{
        AgentSettings, Heartbeat, Hello, PROTOCOL_MAX, PROTOCOL_MIN, PROTOCOL_VERSION,
        bundled::{
            EmbeddedAgentManifest, EmbeddedComponentRecord, encode_embedded_agent_manifest_frame,
        },
        release::{
            RELEASE_SCHEMA, RELEASE_SOURCE_REPO, ReleaseArtifact, ReleaseMetadata, ReleaseProof,
            TrustedKeys, canonical_asset_name, canonical_path, verify_release,
        },
    };
    use base64::engine::general_purpose::STANDARD;
    use blake2::{Blake2b512, Digest};
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::{Value, json};
    use sha2::Sha256;
    use sqlx::Row;
    use std::{collections::BTreeMap, path::PathBuf};

    const AGENT_VERSION: &str = "0.4.0";
    const RELEASE_TAG: &str = "agent-v0.4.0";
    const TARGET: &str = "linux-musl-amd64";
    const SINGBOX_VERSION: &str = "1.14.4-apsp.2";
    const IPQUALITY_VERSION: &str = "87397e2c3196ec796f5477c83343c2354df601ea-node-r1";
    const SINGBOX_SHA256: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const IPQUALITY_SHA256: &str =
        "2222222222222222222222222222222222222222222222222222222222222222";
    const INSTALLER: &[u8] = b"#!/bin/sh\nexit 0\n";

    struct Directory(PathBuf);

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct TestReports;

    static TEST_REPORTS: TestReports = TestReports;

    impl DeviceReports for TestReports {
        fn hello<'a>(
            &'a self,
            state: &'a AppState,
            server_id: i64,
            capabilities: Vec<String>,
            _applied: AppliedRevisions,
        ) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async move {
                crate::servers::record_capabilities(state, server_id, &capabilities).await
            })
        }

        fn heartbeat<'a>(
            &'a self,
            state: &'a AppState,
            server_id: i64,
            _applied: AppliedRevisions,
        ) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(crate::servers::record_heartbeat(state, server_id))
        }

        fn static_info<'a>(
            &'a self,
            state: &'a AppState,
            server_id: i64,
            info: StaticInfo,
        ) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(crate::servers::record_static_info(state, server_id, info))
        }

        fn metrics<'a>(
            &'a self,
            _state: &'a AppState,
            _server_id: i64,
            _metrics: Metrics,
        ) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn apply_result<'a>(
            &'a self,
            _state: &'a AppState,
            _server_id: i64,
            _result: ApplyResult,
        ) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn checkpoint_result<'a>(
            &'a self,
            _state: &'a AppState,
            _server_id: i64,
            result: RuntimeCheckpointResult,
        ) -> BoxFuture<'a, anyhow::Result<RuntimeControlAck>> {
            Box::pin(async move {
                Ok(RuntimeControlAck {
                    request_id: result.request_id,
                    request_digest: result.request_digest,
                })
            })
        }

        fn path_probe_result<'a>(
            &'a self,
            _state: &'a AppState,
            _server_id: i64,
            result: RuntimePathProbeResult,
        ) -> BoxFuture<'a, anyhow::Result<RuntimeControlAck>> {
            Box::pin(async move {
                Ok(RuntimeControlAck {
                    request_id: result.request_id,
                    request_digest: result.request_digest,
                })
            })
        }

        fn barrier_result<'a>(
            &'a self,
            _state: &'a AppState,
            _server_id: i64,
            result: RuntimeRecoveryBarrierResult,
        ) -> BoxFuture<'a, anyhow::Result<RuntimeControlAck>> {
            Box::pin(async move {
                Ok(RuntimeControlAck {
                    request_id: result.request_id,
                    request_digest: result.request_digest,
                })
            })
        }

        fn diagnostic_cancel_result<'a>(
            &'a self,
            _state: &'a AppState,
            _server_id: i64,
            _result: DiagnosticCancelResult,
        ) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn usage_batch<'a>(
            &'a self,
            _state: &'a AppState,
            _server_id: i64,
            _batch: UsageBatch,
        ) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn runtime_sessions<'a>(
            &'a self,
            _state: &'a AppState,
            _server_id: i64,
            _report: RuntimeSessionReport,
        ) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn retirement_result<'a>(
            &'a self,
            _state: &'a AppState,
            _server_id: i64,
            _result: RetirementResult,
        ) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    struct SignedFixture {
        keys: TrustedKeys,
        proof: ReleaseProof,
        asset_name: String,
        agent_bytes: Vec<u8>,
        agent_sha256: String,
    }

    impl SignedFixture {
        fn build() -> anyhow::Result<Self> {
            let manifest = EmbeddedAgentManifest {
                schema: 1,
                agent_version: AGENT_VERSION.into(),
                target_os: "linux".into(),
                target_arch: "x86_64".into(),
                target_env: "musl".into(),
                components: BTreeMap::from([
                    (
                        "sing-box".into(),
                        EmbeddedComponentRecord {
                            format: "tar.gz".into(),
                            inventory_sha256: None,
                            sha256: Some(SINGBOX_SHA256.into()),
                            subcommand: None,
                            version: SINGBOX_VERSION.into(),
                        },
                    ),
                    (
                        "ipquality".into(),
                        EmbeddedComponentRecord {
                            format: "tar.gz".into(),
                            inventory_sha256: None,
                            sha256: Some(IPQUALITY_SHA256.into()),
                            subcommand: None,
                            version: IPQUALITY_VERSION.into(),
                        },
                    ),
                ]),
            };
            let frame =
                encode_embedded_agent_manifest_frame(&manifest).map_err(anyhow::Error::msg)?;
            let mut agent_bytes = b"\x7fELF-test-agent-0.4.0 ".to_vec();
            agent_bytes.extend_from_slice(&frame);
            agent_bytes.extend_from_slice(b" trailing-test-bytes");
            let agent_sha256 = sha256_hex(&agent_bytes);
            let mut artifact = ReleaseArtifact {
                name: "agent".into(),
                version: AGENT_VERSION.into(),
                arch: TARGET.into(),
                format: "raw".into(),
                binary_name: "apsp-agent".into(),
                archive_size: agent_bytes.len() as u64,
                binary_sha256: agent_sha256.clone(),
                binary_size: agent_bytes.len() as u64,
                asset_name: String::new(),
                auxiliary_files: BTreeMap::new(),
            };
            artifact.asset_name = canonical_asset_name(&artifact)?;
            let asset_name = artifact.asset_name.clone();
            let artifact_path = canonical_path("agent", AGENT_VERSION, TARGET)?;
            let metadata = ReleaseMetadata {
                schema: RELEASE_SCHEMA,
                source_repo: RELEASE_SOURCE_REPO.into(),
                tag: RELEASE_TAG.into(),
                protocol_min: PROTOCOL_MIN,
                protocol_max: PROTOCOL_MAX,
                artifacts: vec![artifact],
            };
            let metadata_json = serde_json::to_string(&metadata)?;
            let checksums: String = BTreeMap::from([
                (artifact_path, agent_sha256.clone()),
                ("install.sh".into(), sha256_hex(INSTALLER)),
                ("release.json".into(), sha256_hex(metadata_json.as_bytes())),
            ])
            .into_iter()
            .map(|(path, sum)| format!("{sum}  {path}\n"))
            .collect();
            let signing = SigningKey::from_bytes(&[0x5a; 32]);
            let key_id = b"TESTCHAN";
            let mut public_record = b"Ed".to_vec();
            public_record.extend_from_slice(key_id);
            public_record.extend_from_slice(signing.verifying_key().as_bytes());
            let keys =
                TrustedKeys::from_json(&serde_json::to_string(&[STANDARD.encode(public_record)])?)?;
            let sig = signing.sign(&Blake2b512::digest(checksums.as_bytes()));
            let comment = "TEST_ONLY device_channel signed release fixture";
            let mut sig_record = b"ED".to_vec();
            sig_record.extend_from_slice(key_id);
            sig_record.extend_from_slice(&sig.to_bytes());
            let mut global = sig.to_bytes().to_vec();
            global.extend_from_slice(comment.as_bytes());
            let global_sig = signing.sign(&global);
            let signature = format!(
                "untrusted comment: signature from minisign secret key\n{}\ntrusted comment: {comment}\n{}\n",
                STANDARD.encode(sig_record),
                STANDARD.encode(global_sig.to_bytes())
            );
            Ok(Self {
                keys,
                proof: ReleaseProof {
                    metadata_json,
                    checksums,
                    signature,
                },
                asset_name,
                agent_bytes,
                agent_sha256,
            })
        }

        fn static_info(&self, hostname: &str) -> StaticInfo {
            StaticInfo {
                os: Some("linux".into()),
                libc: Some("musl".into()),
                arch: Some("x86_64".into()),
                hostname: Some(hostname.into()),
                ip_addresses: vec!["192.0.2.10".into()],
                agent_version: Some(AGENT_VERSION.into()),
                components: BTreeMap::from([
                    ("sing-box".into(), SINGBOX_VERSION.into()),
                    ("ipquality".into(), IPQUALITY_VERSION.into()),
                ]),
                component_digests: BTreeMap::from([
                    ("agent".into(), self.agent_sha256.clone()),
                    ("sing-box".into(), SINGBOX_SHA256.into()),
                    ("ipquality".into(), IPQUALITY_SHA256.into()),
                ]),
                ..StaticInfo::default()
            }
        }
    }

    async fn test_state(
        pool: sqlx::PgPool,
    ) -> anyhow::Result<(AppState, Directory, SignedFixture)> {
        install(&TEST_REPORTS);
        let fixture = SignedFixture::build()?;
        let directory = Directory(
            std::env::temp_dir().join(format!("apsp-device-channel-test-{}", Uuid::new_v4())),
        );
        std::fs::create_dir_all(&directory.0)?;
        let mut state = AppState::new(
            pool,
            Config {
                database_url: String::new(),
                listen: "127.0.0.1:0".parse()?,
                public_url: "http://127.0.0.1".into(),
                data_dir: directory.0.clone(),
                admin_password: None,
            },
        )
        .await?;
        state.release_keys = Some(Arc::new(fixture.keys.clone()));
        let targets = [TARGET.to_owned()];
        let asset_name = fixture.asset_name.clone();
        let agent_bytes = fixture.agent_bytes.clone();
        crate::releases::import_bundle_for_targets(
            &state,
            RELEASE_TAG,
            fixture.proof.clone(),
            &targets,
            move |name, _max| {
                let asset_name = asset_name.clone();
                let agent_bytes = agent_bytes.clone();
                async move {
                    if name == "install.sh" {
                        Ok(INSTALLER.to_vec())
                    } else if name == asset_name {
                        Ok(agent_bytes)
                    } else {
                        anyhow::bail!("unexpected asset {name}")
                    }
                }
            },
        )
        .await?;
        Ok((state, directory, fixture))
    }

    async fn enroll_test_server(pool: &sqlx::PgPool) -> anyhow::Result<(i64, String)> {
        let device_key = SigningKey::from_bytes(&[0x33; 32]);
        let public_key = URL_SAFE_NO_PAD.encode(device_key.verifying_key().as_bytes());
        let server_id: i64 = sqlx::query_scalar(
            "INSERT INTO servers(name,device_public_key) VALUES('TEST_ONLY channel identity',$1) RETURNING id",
        )
        .bind(&public_key)
        .fetch_one(pool)
        .await?;
        Ok((server_id, public_key))
    }

    async fn connect_session(
        state: &AppState,
        server_id: i64,
        public_key: &str,
        token: &str,
    ) -> anyhow::Result<(Uuid, mpsc::Receiver<Envelope>)> {
        let connection_id = Uuid::new_v4();
        let (sender, receiver) = mpsc::channel(8);
        register_connection(
            state,
            server_id,
            public_key,
            token,
            now_timestamp() + 3600,
            AgentConnection {
                id: connection_id,
                sender,
            },
        )
        .await?;
        Ok((connection_id, receiver))
    }

    async fn verify_server_component(
        state: &AppState,
        server_id: i64,
        component: &str,
        expected_version: &str,
    ) -> ApiResult<crate::releases::VerifiedRunningComponent> {
        let row = sqlx::query(
            "SELECT static_info,static_info_received_at,last_seen FROM servers WHERE id=$1 AND deleted_at IS NULL",
        )
        .bind(server_id)
        .fetch_one(&state.pool)
        .await?;
        let static_info: Value = row.get("static_info");
        let received_at: Option<i64> = row.get("static_info_received_at");
        let last_seen: Option<i64> = row.get("last_seen");
        crate::releases::verify_running_component(
            state,
            &static_info,
            received_at,
            last_seen,
            now_timestamp(),
            component,
            expected_version,
        )
        .await
    }

    #[test]
    fn signed_release_fixture_passes_protocol_verification() -> anyhow::Result<()> {
        let fixture = SignedFixture::build()?;
        let verified = verify_release(&fixture.proof, &fixture.keys)?;
        assert_eq!(verified.metadata().tag, RELEASE_TAG);
        let parsed = crate::releases::parse_embedded_manifest_frame(
            &fixture.agent_bytes,
            AGENT_VERSION,
            TARGET,
        )?
        .expect("embedded manifest frame");
        assert_eq!(parsed.components["sing-box"].version, SINGBOX_VERSION);
        assert_eq!(parsed.components["ipquality"].version, IPQUALITY_VERSION);
        Ok(())
    }

    #[sqlx::test(migrations = "../panel/migrations")]
    async fn expired_device_sessions_are_purged_and_live_ones_kept(
        pool: sqlx::PgPool,
    ) -> anyhow::Result<()> {
        let server: i64 =
            sqlx::query_scalar("INSERT INTO servers(name) VALUES('TEST_ONLY') RETURNING id")
                .fetch_one(&pool)
                .await?;
        for (token, expires_at) in [("expired", 100_i64), ("expired-now", 300), ("live", 301)] {
            sqlx::query(
                "INSERT INTO device_sessions(token_hash,server_id,expires_at) VALUES($1,$2,$3)",
            )
            .bind(tokens::hash_token(token))
            .bind(server)
            .bind(expires_at)
            .execute(&pool)
            .await?;
        }
        assert_eq!(purge_expired_sessions(&pool, 300).await?, 2);
        let remaining: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM device_sessions")
            .fetch_all(&pool)
            .await?;
        assert_eq!(remaining, [tokens::hash_token("live")]);
        Ok(())
    }

    #[sqlx::test(migrations = "../panel/migrations")]
    async fn new_auth_clears_previous_static_eligibility_and_keeps_readonly_history(
        pool: sqlx::PgPool,
    ) -> anyhow::Result<()> {
        let (state, _directory, fixture) = test_state(pool.clone()).await?;
        let (server_id, public_key) = enroll_test_server(&pool).await?;

        let (conn_x, _rx_x) =
            connect_session(&state, server_id, &public_key, "session-token-x").await?;
        let hello = Message::Hello(Hello {
            agent_version: AGENT_VERSION.into(),
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec![apsp_protocol::SELF_CONTAINED_AGENT_CAPABILITY.into()],
            applied: BTreeMap::new(),
        });
        assert!(process_connection_message(&state, server_id, conn_x, hello).await?);
        let static_x = Message::TelemetryStatic(fixture.static_info("node-old-process"));
        assert!(process_connection_message(&state, server_id, conn_x, static_x).await?);
        let verified_x =
            verify_server_component(&state, server_id, "sing-box", SINGBOX_VERSION).await?;
        assert_eq!(verified_x.agent_sha256, fixture.agent_sha256);

        // Process Y authenticates while X's static_info_received_at is still fresh (< 600s),
        // before Y has sent its own TelemetryStatic.
        let (conn_y, _rx_y) =
            connect_session(&state, server_id, &public_key, "session-token-y").await?;
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer session-token-y".parse()?);
        assert_eq!(require_agent(&state, &headers).await?, server_id);
        assert_eq!(
            devices(&state)
                .connections
                .read()
                .await
                .get(&server_id)
                .map(|entry| entry.id),
            Some(conn_y)
        );

        let received_at: Option<i64> =
            sqlx::query_scalar("SELECT static_info_received_at FROM servers WHERE id=$1")
                .bind(server_id)
                .fetch_one(&pool)
                .await?;
        assert!(
            received_at.is_none(),
            "new authentication must clear previous connection's static_info freshness eligibility"
        );
        assert!(
            matches!(
                verify_server_component(&state, server_id, "sing-box", SINGBOX_VERSION).await,
                Err(ApiError::Conflict(_))
            ),
            "high-risk component verification must wait for this connection's fresh static identity"
        );

        // Historical static_info remains read-only for server display, and normal config saving works.
        let query =
            format!("SELECT {SERVER_COLUMNS} FROM servers WHERE id=$1 AND deleted_at IS NULL");
        let server_view = sqlx::query_as::<_, Server>(&query)
            .bind(server_id)
            .fetch_one(&pool)
            .await?
            .with_online();
        assert!(server_view.online);
        assert_eq!(server_view.static_info["hostname"], "node-old-process");
        assert_eq!(
            server_view.static_info["ip_addresses"],
            json!(["192.0.2.10"])
        );

        let updated_settings = AgentSettings {
            sample_interval_secs: 5,
            upload_interval_secs: 15,
            auto_update: true,
            discover_public_ips: false,
        };
        crate::servers::save_agent_settings(&pool, server_id, &updated_settings).await?;
        let saved: Value = sqlx::query_scalar("SELECT agent_settings FROM servers WHERE id=$1")
            .bind(server_id)
            .fetch_one(&pool)
            .await?;
        assert_eq!(
            serde_json::from_value::<AgentSettings>(saved)?,
            updated_settings
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../panel/migrations")]
    async fn delayed_static_or_disconnect_from_obsolete_connection_cannot_authorize_current(
        pool: sqlx::PgPool,
    ) -> anyhow::Result<()> {
        let (state, _directory, fixture) = test_state(pool.clone()).await?;
        let (server_id, public_key) = enroll_test_server(&pool).await?;

        let (conn_x, _rx_x) =
            connect_session(&state, server_id, &public_key, "session-stale-x").await?;
        let initial = Message::TelemetryStatic(fixture.static_info("before-reconnect"));
        assert!(process_connection_message(&state, server_id, conn_x, initial).await?);

        let (conn_y, _rx_y) =
            connect_session(&state, server_id, &public_key, "session-current-y").await?;

        // Delayed static report from obsolete connection X must be rejected and must not
        // repopulate static_info_received_at or overwrite static_info for connection Y.
        let accepted = process_connection_message(
            &state,
            server_id,
            conn_x,
            Message::TelemetryStatic(fixture.static_info("delayed-from-obsolete-x")),
        )
        .await?;
        assert!(
            !accepted,
            "obsolete connection static report must not be accepted"
        );

        let row = sqlx::query(
            "SELECT static_info,static_info_received_at,last_seen FROM servers WHERE id=$1",
        )
        .bind(server_id)
        .fetch_one(&pool)
        .await?;
        assert!(
            row.get::<Option<i64>, _>("static_info_received_at")
                .is_none()
        );
        assert_eq!(
            row.get::<Value, _>("static_info")["hostname"],
            "before-reconnect"
        );
        assert!(matches!(
            verify_server_component(&state, server_id, "ipquality", IPQUALITY_VERSION).await,
            Err(ApiError::Conflict(_))
        ));

        // Delayed disconnect cleanup of obsolete connection X must neither remove Y nor backdate last_seen.
        unregister_connection(&state, server_id, conn_x).await?;
        assert_eq!(
            devices(&state)
                .connections
                .read()
                .await
                .get(&server_id)
                .map(|entry| entry.id),
            Some(conn_y)
        );
        let last_seen: Option<i64> =
            sqlx::query_scalar("SELECT last_seen FROM servers WHERE id=$1")
                .bind(server_id)
                .fetch_one(&pool)
                .await?;
        assert!(
            last_seen.is_some_and(|seen| now_timestamp().saturating_sub(seen) <= 60),
            "obsolete connection cleanup must leave the active connection online"
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../panel/migrations")]
    async fn current_connection_static_unblocks_matching_package_rejects_drift_and_keeps_heartbeat(
        pool: sqlx::PgPool,
    ) -> anyhow::Result<()> {
        let (state, _directory, fixture) = test_state(pool.clone()).await?;
        let (server_id, public_key) = enroll_test_server(&pool).await?;

        // Old connection X establishes and freezes verified package identity.
        let (conn_x, _rx_x) =
            connect_session(&state, server_id, &public_key, "session-freeze-x").await?;
        let static_x = Message::TelemetryStatic(fixture.static_info("node-x"));
        assert!(process_connection_message(&state, server_id, conn_x, static_x).await?);
        let frozen =
            verify_server_component(&state, server_id, "ipquality", IPQUALITY_VERSION).await?;

        // New connection Y authenticates; heartbeats before static report must not restore eligibility.
        let (conn_y, mut rx_y) =
            connect_session(&state, server_id, &public_key, "session-live-y").await?;
        let early_beat = Message::Heartbeat(Heartbeat {
            applied: BTreeMap::new(),
            uptime_secs: 10,
        });
        assert!(process_connection_message(&state, server_id, conn_y, early_beat).await?);
        assert!(matches!(
            verify_server_component(&state, server_id, "ipquality", IPQUALITY_VERSION).await,
            Err(ApiError::Conflict(_))
        ));

        // Once current connection Y reports matching signed static identity, verification succeeds
        // and matches every frozen package identity field.
        let static_y = Message::TelemetryStatic(fixture.static_info("node-y"));
        assert!(process_connection_message(&state, server_id, conn_y, static_y).await?);
        let verified_y =
            verify_server_component(&state, server_id, "ipquality", IPQUALITY_VERSION).await?;
        assert_eq!(verified_y.agent_version, frozen.agent_version);
        assert_eq!(verified_y.release_tag, frozen.release_tag);
        assert_eq!(verified_y.target, frozen.target);
        assert_eq!(verified_y.source_repo, frozen.source_repo);
        assert_eq!(verified_y.agent_sha256, frozen.agent_sha256);
        assert_eq!(verified_y.agent_bytes, frozen.agent_bytes);
        assert_eq!(verified_y.component, frozen.component);
        assert_eq!(verified_y.component_version, frozen.component_version);
        assert_eq!(verified_y.component_sha256, frozen.component_sha256);

        // Normal heartbeats and notifications on the same connection preserve static eligibility.
        let beat = Message::Heartbeat(Heartbeat {
            applied: BTreeMap::from([("singbox".into(), 3)]),
            uptime_secs: 30,
        });
        assert!(process_connection_message(&state, server_id, conn_y, beat).await?);
        notify(
            &state,
            server_id,
            Envelope::new(
                "manifest.changed",
                apsp_protocol::ManifestChanged { rev: 3 },
            )?,
        )
        .await;
        assert!(rx_y.try_recv().is_ok());
        let after_heartbeat =
            verify_server_component(&state, server_id, "ipquality", IPQUALITY_VERSION).await?;
        assert_eq!(
            after_heartbeat.static_info_received_at,
            verified_y.static_info_received_at
        );

        // Changed Agent binary digest or changed embedded component digest on Y remains rejected.
        let mut drifted_agent = fixture.static_info("node-y");
        drifted_agent
            .component_digests
            .insert("agent".into(), "f".repeat(64));
        let msg_agent = Message::TelemetryStatic(drifted_agent);
        assert!(process_connection_message(&state, server_id, conn_y, msg_agent).await?);
        assert!(matches!(
            verify_server_component(&state, server_id, "ipquality", IPQUALITY_VERSION).await,
            Err(ApiError::Conflict(_))
        ));

        let mut drifted_component = fixture.static_info("node-y");
        drifted_component
            .component_digests
            .insert("ipquality".into(), "e".repeat(64));
        let msg_component = Message::TelemetryStatic(drifted_component);
        assert!(process_connection_message(&state, server_id, conn_y, msg_component).await?);
        assert!(matches!(
            verify_server_component(&state, server_id, "ipquality", IPQUALITY_VERSION).await,
            Err(ApiError::Conflict(_))
        ));
        Ok(())
    }
}

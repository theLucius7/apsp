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
    sqlx::query_scalar::<_, i64>("SELECT sessions.server_id FROM sessions JOIN servers ON servers.id = sessions.server_id WHERE sessions.token_hash = $1 AND sessions.server_id IS NOT NULL AND sessions.expires_at > $2 AND servers.deleted_at IS NULL")
        .bind(tokens::hash_token(token)).bind(now_timestamp()).fetch_optional(&state.pool).await?.ok_or(ApiError::Unauthorized)
}

/// Refuses artifact work for an Agent that cannot verify artifact signatures yet.
pub async fn require_signed_agent(state: &AppState, server_id: i64) -> ApiResult<()> {
    let capabilities: serde_json::Value =
        sqlx::query_scalar("SELECT capabilities FROM servers WHERE id=$1 AND deleted_at IS NULL")
            .bind(server_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(ApiError::Unauthorized)?;
    if !capabilities.as_array().is_some_and(|values| {
        values
            .iter()
            .any(|v| v.as_str() == Some(apsp_protocol::release::ARTIFACT_SIGNATURE_CAPABILITY))
    }) {
        return Err(ApiError::Conflict(
            "此 Agent 尚不支持制品验签，请先使用可信安装器升级；已运行配置和流量上报继续保留"
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
    let devices = devices(&state);
    {
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
            current.as_deref() == Some(&public_key),
            "device was deleted or rebound during authentication"
        );
        sqlx::query("INSERT INTO sessions(token_hash,server_id,expires_at) VALUES($1,$2,$3)")
            .bind(tokens::hash_token(&session_token))
            .bind(server_id)
            .bind(expires_at)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE servers SET last_seen=$2,last_contact_at=$2 WHERE id=$1")
            .bind(server_id)
            .bind(now_timestamp())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        devices.connections.write().await.insert(
            server_id,
            AgentConnection {
                id: connection_id,
                sender,
            },
        );
    }
    let result: anyhow::Result<()> = async {
        send(&mut socket, Envelope::new("hello.ack", HelloAck {
            server_time, session_token, session_expires_at: expires_at,
        })?).await?;
        let (mut sink, mut stream) = socket.split();
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        let mut last_received = Instant::now();
        let mut introduced = false;
        loop {
            tokio::select! {
                frame = stream.next() => {
                    let Some(frame) = frame else { break; };
                    let frame = frame?;
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
                            match message {
                                // The device keeps a rejected batch unacknowledged in its durable
                                // outbox and replays it. Tearing down the channel here would let one
                                // invalid batch block later batches and every other control message.
                                Message::UsageBatch(batch) => {
                                    if let Err(error) = usage_batch(&state, server_id, batch).await {
                                        tracing::warn!(server_id, %error, "usage batch rejected and left unacknowledged");
                                    }
                                }
                                message => process_message(&state, server_id, message).await?,
                            }
                        }
                        WsMessage::Ping(bytes) => sink.send(WsMessage::Pong(bytes)).await?,
                        WsMessage::Close(_) => break,
                        WsMessage::Pong(_) => {},
                        _ => anyhow::bail!("expected text message"),
                    }
                    sqlx::query("UPDATE servers SET last_seen=$2,last_contact_at=$2 WHERE id=$1 AND deleted_at IS NULL").bind(server_id).bind(now_timestamp()).execute(&state.pool).await?;
                }
                notification = receiver.recv() => {
                    let Some(notification) = notification else { break; };
                    sink.send(WsMessage::Text(serde_json::to_string(&notification)?.into())).await?;
                }
                _ = heartbeat.tick() => {
                    if last_received.elapsed() > Duration::from_secs(60) || now_timestamp() >= expires_at { break; }
                    sink.send(WsMessage::Ping(Vec::new().into())).await?;
                }
            }
        }
        Ok(())
    }.await;
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
    result
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

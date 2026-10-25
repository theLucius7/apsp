use super::Runtime;
use crate::{
    Config, artifacts::PanelClient, config::validate_panel_url, identity::Identity,
    state::StorageRetry,
};
use anyhow::{Context, Result};
use apsp_protocol::{
    AuthResponse, Envelope, Heartbeat, Hello, HelloAck, Message, PROTOCOL_VERSION,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::Signer;
use futures_util::{SinkExt, StreamExt};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use tokio::{
    net::TcpStream,
    sync::{mpsc, watch},
    time::{Instant, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{Message as Frame, protocol::WebSocketConfig},
};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub(super) async fn run(
    config: &Config,
    identity: &Identity,
    runtime: &Runtime,
    client_tx: &watch::Sender<Option<Arc<PanelClient>>>,
    trigger: &mpsc::Sender<()>,
    outgoing: &mut mpsc::Receiver<Envelope>,
) -> Result<()> {
    let (mut socket, ack) = authenticate(config, identity).await?;
    send(
        &mut socket,
        Envelope::new(
            "hello",
            Hello {
                agent_version: runtime.agent_version.into(),
                protocol_version: PROTOCOL_VERSION,
                capabilities: runtime.capabilities.as_ref().clone(),
                applied: runtime.applied()?,
            },
        )?,
    )
    .await?;
    let mut last_static = runtime.static_info()?;
    if let Some(info) = &last_static {
        send(&mut socket, Envelope::new("telemetry.static", info)?).await?;
    }
    client_tx.send_replace(Some(Arc::new(PanelClient::from_config(
        config,
        &ack.session_token,
    )?)));
    runtime.connected.store(true, Ordering::Relaxed);
    let mut clock_storage = StorageRetry::default();
    let mut acknowledgment_storage = StorageRetry::default();
    let clock_offset = ack
        .server_time
        .saturating_mul(1000)
        .saturating_sub(apsp_protocol::telemetry::now_millis());
    let mut clock_pending = {
        let mut state = runtime
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        clock_storage
            .finish(
                "persist authenticated clock offset",
                state.set_json("clock_offset_ms", &clock_offset),
            )?
            .is_none()
    };
    let _ = trigger.try_send(());
    let renew_after = ack
        .session_expires_at
        .saturating_sub(ack.server_time)
        .saturating_sub(60)
        .clamp(1, 3540) as u64;
    let renewal = tokio::time::sleep(Duration::from_secs(renew_after));
    tokio::pin!(renewal);
    let mut heartbeat = interval(20);
    let mut resend = interval(15);
    let mut stale = interval(10);
    let mut static_refresh = interval(300);
    let mut retirement_poll = interval(5);
    let mut telemetry = runtime.telemetry.clone();
    let mut cache_open = true;
    let mut last_received = Instant::now();
    let mut resend_queue = std::collections::VecDeque::new();
    let mut resend_deadline = Instant::now();
    loop {
        tokio::select! {
            biased;
            incoming = socket.next() => {
                let frame = incoming.context("panel WebSocket closed")??;
                last_received = Instant::now();
                match frame {
                    Frame::Text(text) => {
                        let envelope: Envelope = serde_json::from_str(&text)?;
                        anyhow::ensure!(envelope.v == PROTOCOL_VERSION, "incompatible panel protocol version");
                        match envelope.decode()? {
                            Message::RuntimeCheckpointRequest(request) => {
                                if let Some(control) = &runtime.runtime_control {
                                    control.enqueue(super::runtime_control::Input::Request(crate::state::runtime_control::ControlRequest::Checkpoint(request)));
                                }
                            }
                            Message::RuntimeRecoveryBarrierRequest(request) => {
                                if let Some(control) = &runtime.runtime_control {
                                    control.enqueue(super::runtime_control::Input::Request(crate::state::runtime_control::ControlRequest::Barrier(request)));
                                }
                            }
                            Message::RuntimePathProbeRequest(request) => {
                                if let Some(control) = &runtime.runtime_control {
                                    control.enqueue(super::runtime_control::Input::Request(crate::state::runtime_control::ControlRequest::Probe(request)));
                                }
                            }
                            Message::RuntimePathProbeAck(ack) => {
                                if let Some(control) = &runtime.runtime_control {
                                    control.enqueue(super::runtime_control::Input::Ack("probe", ack));
                                }
                            }
                            Message::RuntimeCheckpointAck(ack) => {
                                if let Some(control) = &runtime.runtime_control {
                                    control.enqueue(super::runtime_control::Input::Ack("checkpoint", ack));
                                }
                            }
                            Message::RuntimeRecoveryBarrierAck(ack) => {
                                if let Some(control) = &runtime.runtime_control {
                                    control.enqueue(super::runtime_control::Input::Ack("barrier", ack));
                                }
                            }
                            Message::DiagnosticCancelRequest(request) => {
                                let _guard = match &runtime.retirement {
                                    Some(retirement) => Some(retirement.gate.read().await),
                                    None => None,
                                };
                                if runtime.retirement.as_ref().is_some_and(|retirement| retirement.requested()) {
                                    continue;
                                }
                                if let Some(control) = &runtime.cancellation {
                                    if let Err(error) = control.request(request) {
                                        tracing::warn!(%error, "diagnostic cancellation request was rejected");
                                    }
                                } else { tracing::warn!("confirmed diagnostic cancellation is not supported"); }
                            }
                            Message::RetirementRequest(request) => {
                                let retirement = runtime.retirement.as_ref().context("retirement worker missing")?;
                                let request_id = request.request_id;
                                if let Err(error) = retirement.request(identity, request) {
                                    send(&mut socket, Envelope::new("retirement.result", apsp_protocol::RetirementResult {
                                        request_id, success: false, error: Some(error.to_string()), receipt: None,
                                    })?).await?;
                                }
                                retirement_poll.reset_immediately();
                            }
                            Message::ManifestChanged(_) => { let _ = trigger.try_send(()); }
                            Message::RuntimeAdmissionPolicy(policy) => {
                                if let Some(admission) = &runtime.admission {
                                    admission.enqueue(policy);
                                }
                            }
                            Message::UsageAck(ack) => {
                                let mut state = runtime.state.lock().map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
                                acknowledgment_storage.finish("persist usage acknowledgment", state.acknowledge_usage(ack.epoch, ack.seq))?;
                            }
                            Message::Unknown { message_type, .. } => tracing::debug!(%message_type, "ignoring unknown panel message"),
                            _ => tracing::debug!("ignoring unsupported panel message direction"),
                        }
                    }
                    Frame::Ping(bytes) => { timeout(Duration::from_secs(10), socket.send(Frame::Pong(bytes))).await??; }
                    Frame::Pong(_) => {}
                    Frame::Close(_) => return Ok(()),
                    _ => anyhow::bail!("panel sent an unsupported frame"),
                }
            }
            _ = retirement_poll.tick(), if runtime.retirement.as_ref().is_some_and(|worker| worker.requested()) => {
                let retirement = runtime.retirement.as_ref().context("retirement worker missing")?;
                let request_id = retirement.request_id()?.context("retirement request missing")?;
                if let Err(error) = retirement.prepare().await {
                    send(&mut socket, Envelope::new("retirement.result", apsp_protocol::RetirementResult {
                        request_id, success: false, error: Some(format!("{error:#}")), receipt: None,
                    })?).await?;
                    continue;
                }
                let pending = runtime.state.lock().map_err(|_| anyhow::anyhow!("state lock poisoned"))?.pending_usage_count()?;
                if pending != 0 {
                    continue;
                }
                let receipt = match retirement.complete(identity).await {
                    Ok(receipt) => receipt,
                    Err(error) => {
                        send(&mut socket, Envelope::new("retirement.result", apsp_protocol::RetirementResult {
                            request_id, success: false, error: Some(format!("{error:#}")), receipt: None,
                        })?).await?;
                        // Completion may have removed the key before an I/O failure. Restart
                        // through durable cleanup recovery rather than authenticating again.
                        return Err(error);
                    }
                };
                client_tx.send_replace(None);
                runtime.connected.store(false, Ordering::Relaxed);
                let _ = send(&mut socket, Envelope::new("retirement.result", apsp_protocol::RetirementResult {
                    request_id, success: true, error: None, receipt: Some(receipt),
                })?).await;
                let _ = timeout(Duration::from_secs(2), socket.close(None)).await;
                retirement.deliver_receipt().await?;
                return Err(crate::retirement::Retired.into());
            }
            message = outgoing.recv() => {
                send(&mut socket, message.context("runtime result channel closed")?).await?;
            }
            _ = heartbeat.tick() => {
                if clock_pending {
                    let mut state = runtime.state.lock().map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
                    clock_pending = clock_storage.finish("persist authenticated clock offset", state.set_json("clock_offset_ms", &clock_offset))?.is_none();
                }
                let uptime = telemetry.borrow().sample.as_ref().and_then(|sample| sample.metrics.uptime_secs).unwrap_or(0);
                send(&mut socket, Envelope::new("heartbeat", Heartbeat {
                    applied: runtime.applied()?,
                    uptime_secs: uptime,
                })?).await?;
            }
            _ = static_refresh.tick() => {
                if let Some(info) = runtime.static_info()? {
                    send(&mut socket, Envelope::new("telemetry.static", &info)?).await?;
                    last_static = Some(info);
                }
            }
            changed = telemetry.changed(), if cache_open => {
                if changed.is_err() {
                    cache_open = false;
                } else if let Some(info) = runtime.static_info()?
                    && Some(&info) != last_static.as_ref() {
                    send(&mut socket, Envelope::new("telemetry.static", &info)?).await?;
                    last_static = Some(info);
                }
            }
            _ = resend.tick() => {
                if resend_queue.is_empty() {
                    let state = runtime.state.lock().map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
                    let oversized = state.oversized_usage_count()?;
                    if oversized != 0 {
                        tracing::error!(oversized_batches = oversized,
                            "legacy usage batches exceed the wire byte budget; preserved for ledger reconciliation");
                    }
                    resend_queue.extend(state.pending_usage()?);
                }
                resend_deadline = Instant::now() + Duration::from_secs(1);
            }
            _ = stale.tick() => {
                anyhow::ensure!(last_received.elapsed() <= Duration::from_secs(60), "panel connection timed out");
            }
            _ = &mut renewal => {
                let _ = timeout(Duration::from_secs(2), socket.close(None)).await;
                return Ok(());
            }
            _ = std::future::ready(()), if !resend_queue.is_empty() && Instant::now() < resend_deadline => {
                // Yield to input and timers between batches even when catching up after an outage.
                let batch = resend_queue.pop_front().context("resend queue unexpectedly empty")?;
                send(&mut socket, Envelope::new("usage.batch", batch)?).await?;
                tokio::task::yield_now().await;
            }
        }
    }
}

fn interval(seconds: u64) -> tokio::time::Interval {
    let mut timer = tokio::time::interval(Duration::from_secs(seconds));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    timer
}

async fn authenticate(config: &Config, identity: &Identity) -> Result<(Socket, HelloAck)> {
    let mut url = validate_panel_url(&config.panel_url)?.join("api/agent/v1/ws")?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme)
        .map_err(|_| anyhow::anyhow!("invalid WebSocket URL"))?;
    let options = WebSocketConfig::default()
        .max_message_size(Some(1024 * 1024))
        .max_frame_size(Some(1024 * 1024));
    let (mut socket, _) = timeout(
        Duration::from_secs(config.operation_timeout_secs),
        connect_async_tls_with_config(
            url.as_str(),
            Some(options),
            false,
            crate::panel_tls::websocket_connector(config.panel_ca_file.as_deref())?,
        ),
    )
    .await??;
    let challenge = receive(&mut socket).await?;
    let Message::AuthChallenge(challenge) = challenge.decode()? else {
        anyhow::bail!("expected panel authentication challenge");
    };
    anyhow::ensure!(
        !challenge.nonce.is_empty() && challenge.nonce.len() <= 512,
        "invalid authentication challenge"
    );
    let signature = identity.signing_key.sign(challenge.nonce.as_bytes());
    send(
        &mut socket,
        Envelope::new(
            "auth.response",
            AuthResponse {
                server_id: identity.server_id,
                signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
            },
        )?,
    )
    .await?;
    let ack = receive(&mut socket).await?;
    let Message::HelloAck(ack) = ack.decode()? else {
        anyhow::bail!("panel rejected authentication");
    };
    anyhow::ensure!(
        !ack.session_token.is_empty()
            && ack.session_token.len() <= 512
            && ack.session_expires_at > ack.server_time,
        "invalid panel session"
    );
    Ok((socket, ack))
}

async fn receive(socket: &mut Socket) -> Result<Envelope> {
    timeout(Duration::from_secs(10), async {
        loop {
            match socket
                .next()
                .await
                .context("panel closed authentication connection")??
            {
                Frame::Text(text) => {
                    let envelope: Envelope = serde_json::from_str(&text)?;
                    anyhow::ensure!(
                        envelope.v == PROTOCOL_VERSION,
                        "incompatible panel protocol version"
                    );
                    return Ok(envelope);
                }
                Frame::Ping(bytes) => socket.send(Frame::Pong(bytes)).await?,
                Frame::Pong(_) => {}
                _ => anyhow::bail!("expected authentication envelope"),
            }
        }
    })
    .await?
}

async fn send(socket: &mut Socket, envelope: Envelope) -> Result<()> {
    let encoded = serde_json::to_string(&envelope)?;
    anyhow::ensure!(
        encoded.len() < 1024 * 1024,
        "outgoing message exceeds protocol limit"
    );
    timeout(
        Duration::from_secs(10),
        socket.send(Frame::Text(encoded.into())),
    )
    .await??;
    Ok(())
}

#[cfg(test)]
mod tests;

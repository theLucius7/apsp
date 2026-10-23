use super::*;
use crate::{State, usage::MAX_PENDING_USAGE_BYTES};
use apsp_protocol::{AuthChallenge, UsageAck, UsageBatch, UsageRecord};
use ed25519_dalek::SigningKey;
use std::{collections::BTreeMap, path::PathBuf, sync::Mutex};
use tokio::{net::TcpListener, task::JoinSet};
use tokio_tungstenite::accept_async;
use uuid::Uuid;

#[cfg(unix)]
mod panel_ca;
mod readiness;
mod runtime_control;
mod storage;

type Peer = WebSocketStream<TcpStream>;

struct Directory(PathBuf);

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn receive_peer(peer: &mut Peer) -> Result<(Envelope, usize)> {
    loop {
        match peer.next().await.context("fixture connection closed")?? {
            Frame::Text(text) => return Ok((serde_json::from_str(&text)?, text.len())),
            Frame::Ping(bytes) => peer.send(Frame::Pong(bytes)).await?,
            other => anyhow::bail!("unexpected fixture frame: {other:?}"),
        }
    }
}

async fn send_peer(peer: &mut Peer, envelope: Envelope) -> Result<()> {
    peer.send(Frame::Text(serde_json::to_string(&envelope)?.into()))
        .await?;
    Ok(())
}

async fn accept_peer(listener: &TcpListener) -> Result<Peer> {
    let (stream, _) = listener.accept().await?;
    let mut peer = accept_async(stream).await?;
    send_peer(
        &mut peer,
        Envelope::new(
            "auth.challenge",
            AuthChallenge {
                nonce: "usage-bound-fixture".into(),
                server_time: 10,
            },
        )?,
    )
    .await?;
    assert!(matches!(
        receive_peer(&mut peer).await?.0.decode()?,
        Message::AuthResponse(_)
    ));
    send_peer(
        &mut peer,
        Envelope::new(
            "hello.ack",
            HelloAck {
                server_time: 10,
                session_token: "fixture-session".into(),
                session_expires_at: 3_610,
            },
        )?,
    )
    .await?;
    Ok(peer)
}

fn spawn_connection(
    tasks: &mut JoinSet<Result<()>>,
    config: Config,
    runtime: Runtime,
) -> mpsc::Receiver<()> {
    let (trigger_tx, trigger_rx) = mpsc::channel(4);
    tasks.spawn(async move {
        let identity = Identity {
            server_id: 1,
            signing_key: SigningKey::from_bytes(&[7; 32]),
        };
        let (client_tx, _client_rx) = watch::channel(None);
        let (_outgoing_tx, mut outgoing_rx) = mpsc::channel(1);
        run(
            &config,
            &identity,
            &runtime,
            &client_tx,
            &trigger_tx,
            &mut outgoing_rx,
        )
        .await
    });
    trigger_rx
}

#[tokio::test]
async fn backlog_and_legacy_giant_preserve_heartbeat_control_ack_and_restart_replay() -> Result<()>
{
    let directory =
        Directory(std::env::temp_dir().join(format!("sn-usage-wire-{}", Uuid::new_v4())));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let config = Config {
        panel_url: format!("http://{}", listener.local_addr()?),

        panel_ca_file: None,
        state_db: directory.0.join("state.db"),
        identity_dir: directory.0.join("identity"),
        runtime_root: directory.0.join("runtime"),
        install_root: directory.0.join("install"),
        agent_root: directory.0.join("agent"),
        status_socket: directory.0.join("status.sock"),
        operation_timeout_secs: 1,
        public_ips: vec![],
        allow_remote_commands: false,
        settings: Default::default(),
    };
    let mut state = State::open(&config.state_db)?;
    let transaction = state.connection.transaction()?;
    transaction.execute(
        "INSERT INTO usage_outbox(epoch,seq,batch) VALUES (?1,'1',?2)",
        rusqlite::params![
            Uuid::nil().to_string(),
            "x".repeat(2 * MAX_PENDING_USAGE_BYTES)
        ],
    )?;
    for seq in 2..=4_097 {
        let batch = UsageBatch {
            epoch: Uuid::from_u128(u128::from(seq)),
            seq,
            period_start: 10,
            period_end: 20,
            records: vec![UsageRecord {
                stat_name: "u1_n1".into(),
                uplink: seq,
                downlink: seq,
            }],
        };
        transaction.execute(
            "INSERT INTO usage_outbox(epoch,seq,batch) VALUES (?1,?2,?3)",
            rusqlite::params![
                batch.epoch.to_string(),
                seq.to_string(),
                serde_json::to_string(&batch)?
            ],
        )?;
    }
    transaction.commit()?;
    let mut runtime = Runtime {
        state: Arc::new(Mutex::new(state)),
        modules: Arc::new(vec![]),
        capabilities: Arc::new(vec![]),
        connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        public_ips: Arc::new(vec![]),
        agent_version: "fixture-agent",
        retirement: None,
        cancellation: None,
        runtime_control: None,
        telemetry: watch::channel(Arc::new(crate::telemetry::cache::Snapshot::default())).1,
    };
    let mut tasks = JoinSet::new();
    spawn_connection(&mut tasks, config.clone(), runtime.clone());
    let mut peer = timeout(Duration::from_secs(5), accept_peer(&listener)).await??;
    let mut rounds = BTreeMap::<u64, usize>::new();
    let mut window_bytes = 0;
    let mut usage_count = 0;
    let mut first_heartbeat = None;
    let mut replay = None;
    timeout(Duration::from_secs(25), async {
        loop {
            let (envelope, bytes) = receive_peer(&mut peer).await?;
            match envelope.decode()? {
                Message::UsageBatch(batch) => {
                    assert!(batch.seq >= 2);
                    window_bytes += bytes;
                    usage_count += 1;
                    *rounds.entry(batch.seq).or_default() += 1;
                    if batch.seq == 2 {
                        send_peer(
                            &mut peer,
                            Envelope::new(
                                "usage.ack",
                                UsageAck {
                                    epoch: batch.epoch,
                                    seq: batch.seq,
                                },
                            )?,
                        )
                        .await?;
                    }
                    if batch.seq == 3 {
                        replay = Some(batch);
                    }
                    if usage_count % crate::usage::MAX_PENDING_USAGE_BATCHES == 0 {
                        assert!(window_bytes <= MAX_PENDING_USAGE_BYTES);
                        window_bytes = 0;
                    }
                }
                Message::Heartbeat(_) => {
                    if let Some(first) = first_heartbeat {
                        let elapsed: Duration = Instant::now() - first;
                        assert!(elapsed <= Duration::from_secs(23));
                        assert_eq!(usage_count, 128);
                        break;
                    }
                    first_heartbeat = Some(Instant::now());
                }
                _ => {}
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    assert_eq!(rounds.get(&2), Some(&1));
    assert_eq!(rounds.get(&3), Some(&2));
    assert!(runtime.connected.load(Ordering::Relaxed));
    assert_eq!(runtime.state.lock().unwrap().pending_usage_count()?, 4_096);
    assert_eq!(runtime.state.lock().unwrap().oversized_usage_count()?, 1);
    peer.send(Frame::Close(None)).await?;
    timeout(Duration::from_secs(5), tasks.join_next())
        .await?
        .context("connection task missing")???;
    // Reopen the persistent database just as a process restart does; the ACK survives.
    runtime.state = Arc::new(Mutex::new(State::open(&config.state_db)?));
    spawn_connection(&mut tasks, config, runtime);
    let mut peer = timeout(Duration::from_secs(5), accept_peer(&listener)).await??;
    let retried = timeout(Duration::from_secs(5), async {
        loop {
            if let Message::UsageBatch(batch) = receive_peer(&mut peer).await?.0.decode()? {
                return Ok::<_, anyhow::Error>(batch);
            }
        }
    })
    .await??;
    assert_eq!(Some(retried), replay);
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

#[tokio::test]
async fn blocked_collection_preserves_real_twenty_second_heartbeat_and_control() -> Result<()> {
    let directory =
        Directory(std::env::temp_dir().join(format!("sn-cache-wire-{}", Uuid::new_v4())));
    let fixture = crate::telemetry::cache::tests::BlockingFixture::new()?;
    let previous = fixture.wait_until_blocked().await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let config = Config {
        panel_url: format!("http://{}", listener.local_addr()?),

        panel_ca_file: None,
        state_db: directory.0.join("state.db"),
        identity_dir: directory.0.join("identity"),
        runtime_root: directory.0.join("runtime"),
        install_root: directory.0.join("install"),
        agent_root: directory.0.join("agent"),
        status_socket: directory.0.join("status.sock"),
        operation_timeout_secs: 1,
        public_ips: vec![],
        allow_remote_commands: false,
        settings: Default::default(),
    };
    let runtime = Runtime {
        state: Arc::new(Mutex::new(State::open(&config.state_db)?)),
        modules: Arc::new(vec![]),
        capabilities: Arc::new(vec![]),
        connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        public_ips: Arc::new(vec![]),
        agent_version: "fixture-agent",
        retirement: None,
        cancellation: None,
        runtime_control: None,
        telemetry: fixture.sampling.snapshots.clone(),
    };
    assert_eq!(
        runtime
            .static_info()?
            .context("fixture snapshot missing")?
            .hostname
            .as_deref(),
        Some("fixture-cached-host")
    );
    let mut tasks = JoinSet::new();
    let mut control = spawn_connection(&mut tasks, config.clone(), runtime.clone());
    let mut peer = timeout(Duration::from_secs(5), accept_peer(&listener)).await??;
    assert_eq!(
        timeout(Duration::from_secs(1), control.recv()).await?,
        Some(())
    );
    let mut first = None;
    timeout(Duration::from_secs(25), async {
        loop {
            if let Message::Heartbeat(heartbeat) = receive_peer(&mut peer).await?.0.decode()? {
                assert_eq!(heartbeat.uptime_secs, 42);
                if let Some(first) = first {
                    assert!((Instant::now() - first) <= Duration::from_secs(23));
                    break;
                }
                first = Some(Instant::now());
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    // A control frame is handled promptly after the sampling timeout, and the
    // cached sample keeps its original UUID and collection timestamp.
    peer.send(Frame::Ping(vec![1, 2, 3].into())).await?;
    timeout(Duration::from_secs(1), async {
        loop {
            if let Frame::Pong(bytes) = peer.next().await.context("fixture closed")?? {
                assert_eq!(bytes.as_ref(), &[1, 2, 3]);
                break Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await??;
    send_peer(
        &mut peer,
        Envelope::new(
            "manifest.changed",
            apsp_protocol::ManifestChanged { rev: 1 },
        )?,
    )
    .await?;
    assert_eq!(
        timeout(Duration::from_secs(1), control.recv()).await?,
        Some(())
    );
    assert!(runtime.connected.load(Ordering::Relaxed));
    assert!(fixture.sampling.snapshots.borrow().timed_out());
    assert_eq!(
        fixture.sampling.snapshots.borrow().sample.as_ref(),
        Some(&previous)
    );
    assert_eq!(fixture.starts.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 2);
    peer.send(Frame::Close(None)).await?;
    timeout(Duration::from_secs(2), tasks.join_next())
        .await?
        .context("connection missing")???;
    // Reconnect consumes the same cache and does not create a second collector.
    spawn_connection(&mut tasks, config, runtime);
    let mut peer = timeout(Duration::from_secs(5), accept_peer(&listener)).await??;
    timeout(Duration::from_secs(2), async {
        loop {
            if let Message::TelemetryStatic(info) = receive_peer(&mut peer).await?.0.decode()? {
                assert_eq!(info.hostname.as_deref(), Some("fixture-cached-host"));
                break Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await??;
    assert_eq!(fixture.starts.load(Ordering::SeqCst), 1);
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

#[tokio::test]
async fn cancellation_request_is_persisted_without_waiting_for_cleanup_or_delaying_heartbeats()
-> Result<()> {
    use crate::transport::diagnostics::cancellation::CancellationControl;
    use apsp_protocol::{Artifact, DiagnosticCancelRequest, DiagnosticJob};
    let directory =
        Directory(std::env::temp_dir().join(format!("sn-cancel-wire-{}", Uuid::new_v4())));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let config = Config {
        panel_url: format!("http://{}", listener.local_addr()?),

        panel_ca_file: None,
        state_db: directory.0.join("state.db"),
        identity_dir: directory.0.join("identity"),
        runtime_root: directory.0.join("runtime"),
        install_root: directory.0.join("install"),
        agent_root: directory.0.join("agent"),
        status_socket: directory.0.join("status.sock"),
        operation_timeout_secs: 1,
        public_ips: vec![],
        allow_remote_commands: false,
        settings: Default::default(),
    };
    let state = Arc::new(Mutex::new(State::open(&config.state_db)?));
    let control = Arc::new(CancellationControl::new(
        state.clone(),
        1,
        vec!["diagnostic-fixture".into()],
    ));
    let runtime = Runtime {
        state: state.clone(),
        modules: Arc::new(vec![]),
        capabilities: Arc::new(vec![apsp_protocol::DIAGNOSTIC_CANCEL_CAPABILITY.into()]),
        connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        public_ips: Arc::new(vec![]),
        agent_version: "fixture-agent",
        retirement: None,
        cancellation: Some(control),
        runtime_control: None,
        telemetry: watch::channel(Arc::new(crate::telemetry::cache::Snapshot::default())).1,
    };
    let mut tasks = JoinSet::new();
    spawn_connection(&mut tasks, config, runtime);
    let mut peer = timeout(Duration::from_secs(5), accept_peer(&listener)).await??;
    timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                receive_peer(&mut peer).await?.0.decode()?,
                Message::Heartbeat(_)
            ) {
                return Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await??;
    let request = DiagnosticCancelRequest {
        server_id: 1,
        job: DiagnosticJob {
            resource_budget: None,
            id: Uuid::from_u128(19),
            plugin: "diagnostic-fixture".into(),
            version: "fixed-version".into(),
            artifact: Artifact {
                url: "https://panel.example.invalid/fixed-artifact".into(),
                sha256: "a".repeat(64),
                proof: None,
            },
            timeout_secs: 1800,
            expires_at: None,
            options: BTreeMap::new(),
        },
    };
    send_peer(
        &mut peer,
        Message::DiagnosticCancelRequest(request.clone()).into_envelope()?,
    )
    .await?;
    let started = Instant::now();
    // No diagnostic cleanup worker exists in this fixture. The next scheduled
    // heartbeat must still arrive while the durable request remains unconfirmed.
    timeout(Duration::from_secs(23), async {
        loop {
            if matches!(
                receive_peer(&mut peer).await?.0.decode()?,
                Message::Heartbeat(_)
            ) {
                return Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await??;
    assert!(started.elapsed() <= Duration::from_secs(23));
    let saved: Vec<DiagnosticCancelRequest> = state
        .lock()
        .unwrap()
        .get_json("diagnostics:cancellations")?
        .context("request was not durably received")?;
    assert_eq!(saved, vec![request]);
    assert!(
        state
            .lock()
            .unwrap()
            .get_json::<serde_json::Value>("diagnostics:cancellation-results")?
            .is_none()
    );
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

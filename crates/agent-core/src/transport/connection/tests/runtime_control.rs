use super::*;
use crate::state::runtime_control::ControlRequest;
use apsp_protocol::{RuntimeBinding, RuntimeCheckpointRequest, RuntimeControlAck};

#[tokio::test]
async fn checkpoint_enqueue_does_not_wait_for_the_control_worker_or_delay_heartbeats() -> Result<()>
{
    let directory =
        Directory(std::env::temp_dir().join(format!("sn-control-wire-{}", Uuid::new_v4())));
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
    let (control, mut pending) = crate::transport::runtime_control::Control::channel();
    let runtime = Runtime {
        state: state.clone(),
        modules: Arc::new(vec![]),
        capabilities: Arc::new(vec![apsp_protocol::RUNTIME_CHECKPOINT_CAPABILITY.into()]),
        connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        public_ips: Arc::new(vec![]),
        agent_version: "fixture-agent",
        retirement: None,
        cancellation: None,
        runtime_control: Some(control),
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
    let request = RuntimeCheckpointRequest {
        request_id: Uuid::new_v4(),
        expected: RuntimeBinding::new(Uuid::new_v4(), "demo".into(), 1, "a".repeat(64)),
        expires_at: apsp_protocol::now_timestamp() + 60,
    };
    send_peer(
        &mut peer,
        Message::RuntimeCheckpointRequest(request.clone()).into_envelope()?,
    )
    .await?;
    let queued = timeout(Duration::from_secs(2), pending.recv())
        .await?
        .context("request was not queued")?;
    assert!(
        matches!(queued, crate::transport::runtime_control::Input::Request(ControlRequest::Checkpoint(value)) if value == request)
    );
    // No worker exists: neither SQLite acknowledgement nor an apply gate was awaited.
    assert!(state.lock().unwrap().pending_runtime_controls()?.is_empty());
    send_peer(
        &mut peer,
        Message::RuntimeCheckpointAck(RuntimeControlAck {
            request_id: request.request_id,
            request_digest: request.digest()?,
        })
        .into_envelope()?,
    )
    .await?;
    let ack = timeout(Duration::from_secs(2), pending.recv())
        .await?
        .context("acknowledgement was not queued")?;
    assert!(
        matches!(ack, crate::transport::runtime_control::Input::Ack("checkpoint", value) if value.request_id == request.request_id)
    );
    let started = Instant::now();
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
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

#[tokio::test]
async fn recovery_floor_refusal_keeps_heartbeat_and_durable_control_failure_available() -> Result<()>
{
    use crate::{
        fake::{FakeAdapter, FakeServiceManager},
        reconcile::{ApplyIntent, Reconciler},
        state::IntentRecord,
        system::SystemOps,
    };
    use apsp_adapter_sdk::{Plan, Prepared, RuntimeSpec};
    let directory =
        Directory(std::env::temp_dir().join(format!("sn-floor-wire-{}", Uuid::new_v4())));
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
    let previous = Prepared {
        spec: RuntimeSpec {
            revision: 1,
            kernel_version: "1".into(),
            config_hash: "a".repeat(64),
            binary_path: config.install_root.join("demo/1/demo"),
            revision_dir: config.runtime_root.join("demo@main/revisions/1"),
            stats_listen: "127.0.0.1:18085".into(),
            files: BTreeMap::new(),
        },
        listen_ports: vec![],
    };
    let mut target = previous.clone();
    target.spec.revision = 3;
    {
        let mut saved = state.lock().unwrap();
        saved.connection.execute(
            "INSERT INTO runtime_revision_floors(module,revision) VALUES ('demo','2')",
            [],
        )?;
        saved.set_json("applied:demo", &previous)?;
        saved.begin_intent(&IntentRecord {
            op_id: Uuid::new_v4(),
            module: "demo".into(),
            payload: serde_json::to_value(ApplyIntent {
                previous: Some(previous),
                target,
                plan: Plan::Restart,
            })?,
        })?;
    }
    let services = Arc::new(FakeServiceManager::default());
    let reconciler = Arc::new(
        Reconciler::new(
            config.clone(),
            state.clone(),
            Arc::new(FakeAdapter::default()),
            Arc::new(SystemOps),
            services.clone(),
        )
        .with_trusted_keys(crate::release_test_support::trusted_keys()),
    );
    let blocked = reconciler.recover().await.unwrap_err();
    assert!(reconciler.management_recovery_allowed(&blocked));
    assert!(!reconciler.management_recovery_allowed(&anyhow::anyhow!("state cannot be opened")));
    let (control, receiver) = crate::transport::runtime_control::Control::channel();
    let runtime = Runtime {
        state: state.clone(),
        modules: Arc::new(vec!["demo".into()]),
        capabilities: Arc::new(vec![]),
        connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        public_ips: Arc::new(vec![]),
        agent_version: "fixture-agent",
        retirement: None,
        cancellation: None,
        runtime_control: Some(control),
        telemetry: watch::channel(Arc::new(crate::telemetry::cache::Snapshot::default())).1,
    };
    let mut tasks = JoinSet::new();
    let (outgoing, _results) = mpsc::channel(64);
    tasks.spawn(crate::transport::runtime_control::run(
        vec![("demo".into(), reconciler)],
        runtime.clone(),
        receiver,
        outgoing,
    ));
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
    // The fixture panel clock is deliberately synthetic; use its authenticated clock.
    let request = RuntimeCheckpointRequest {
        request_id: Uuid::new_v4(),
        expected: RuntimeBinding::new(Uuid::new_v4(), "demo".into(), 1, "a".repeat(64)),
        expires_at: 70,
    };
    send_peer(
        &mut peer,
        Message::RuntimeCheckpointRequest(request).into_envelope()?,
    )
    .await?;
    timeout(Duration::from_secs(3), async {
        loop {
            if let Some(crate::state::runtime_control::ControlResult::Checkpoint(result)) =
                state.lock().unwrap().pending_runtime_results()?.first()
            {
                assert!(!result.success);
                assert!(result.error.as_ref().unwrap().contains("revision floor"));
                break Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
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
    assert!(services.actions.lock().unwrap().is_empty());
    assert_eq!(state.lock().unwrap().pending_intents()?.len(), 1);
    assert_eq!(
        state.lock().unwrap().get_json::<bool>("health:demo")?,
        Some(false)
    );
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

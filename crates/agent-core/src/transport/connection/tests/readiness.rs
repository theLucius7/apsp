use super::*;
use crate::telemetry::cache::Snapshot;
use apsp_protocol::{Metrics, StaticInfo, TelemetrySample};

#[tokio::test]
async fn empty_cache_keeps_host_identity_until_a_real_snapshot_arrives() -> Result<()> {
    let directory =
        Directory(std::env::temp_dir().join(format!("sn-cache-ready-{}", Uuid::new_v4())));
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
    let (snapshots, cache) = watch::channel(Arc::new(Snapshot::default()));
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
        telemetry: cache,
    };
    assert!(runtime.static_info()?.is_none());
    let mut tasks = JoinSet::new();
    let mut control = spawn_connection(&mut tasks, config, runtime);
    let mut peer = timeout(Duration::from_secs(5), accept_peer(&listener)).await??;
    timeout(Duration::from_secs(2), async {
        loop {
            match receive_peer(&mut peer).await?.0.decode()? {
                Message::TelemetryStatic(_) => anyhow::bail!("empty cache overwrote host metadata"),
                Message::Heartbeat(heartbeat) => {
                    assert_eq!(heartbeat.uptime_secs, 0);
                    break Ok::<_, anyhow::Error>(());
                }
                _ => {}
            }
        }
    })
    .await??;
    assert_eq!(
        timeout(Duration::from_secs(1), control.recv()).await?,
        Some(())
    );
    snapshots.send_replace(Arc::new(Snapshot {
        static_info: StaticInfo {
            os: Some("linux".into()),
            arch: Some("x86_64".into()),
            libc: Some("musl".into()),
            runtime_libc: Some("gnu".into()),
            ..Default::default()
        },
        sample: Some(TelemetrySample {
            id: Uuid::new_v4(),
            sampled_at: 123_000,
            metrics: Metrics::default(),
        }),
        started_at: None,
        error: None,
    }));
    timeout(Duration::from_secs(2), async {
        loop {
            if let Message::TelemetryStatic(info) = receive_peer(&mut peer).await?.0.decode()? {
                assert_eq!(info.libc.as_deref(), Some("musl"));
                assert_eq!(info.runtime_libc.as_deref(), Some("gnu"));
                break Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await??;
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

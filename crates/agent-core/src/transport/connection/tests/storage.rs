use super::*;

#[tokio::test]
async fn full_storage_retains_clock_and_unacked_usage_without_interrupting_authenticated_heartbeat()
-> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let config = Config {
        panel_url: format!("http://{}", listener.local_addr()?),
        operation_timeout_secs: 1,
        ..Config::default()
    };
    let mut state = State::open(std::path::Path::new(":memory:"))?;
    state.set_json("clock_offset_ms", &0i64)?;
    let batch = UsageBatch {
        epoch: Uuid::new_v4(),
        seq: 1,
        period_start: 10,
        period_end: 20,
        records: vec![],
    };
    state.connection.execute(
        "INSERT INTO usage_outbox(epoch,seq,batch) VALUES (?1,'1',?2)",
        rusqlite::params![batch.epoch.to_string(), serde_json::to_string(&batch)?],
    )?;
    state.connection.execute_batch(
        "CREATE TABLE TEST_ONLY_full(payload BLOB);
        CREATE TRIGGER TEST_ONLY_clock_full BEFORE INSERT ON kv WHEN NEW.key='clock_offset_ms'
        BEGIN INSERT INTO TEST_ONLY_full VALUES(zeroblob(65536)); END;
        CREATE TRIGGER TEST_ONLY_ack_full BEFORE UPDATE ON usage_outbox
        BEGIN INSERT INTO TEST_ONLY_full VALUES(zeroblob(65536)); END;",
    )?;
    let pages: i64 = state
        .connection
        .query_row("PRAGMA page_count", [], |row| row.get(0))?;
    state
        .connection
        .execute_batch(&format!("PRAGMA max_page_count={pages}"))?;
    let runtime = Runtime {
        state: Arc::new(Mutex::new(state)),
        modules: Arc::new(vec![]),
        capabilities: Arc::new(vec![]),
        connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        public_ips: Arc::new(vec![]),
        agent_version: "TEST_ONLY",
        retirement: None,
        cancellation: None,
        runtime_control: None,
        admission: None,
        telemetry: watch::channel(Arc::new(crate::telemetry::cache::Snapshot::default())).1,
    };
    let mut tasks = JoinSet::new();
    spawn_connection(&mut tasks, config, runtime.clone());
    let mut peer = timeout(Duration::from_secs(5), accept_peer(&listener)).await??;
    let first = timeout(Duration::from_secs(3), async {
        loop {
            if let Message::Heartbeat(_) = receive_peer(&mut peer).await?.0.decode()? {
                break Ok::<_, anyhow::Error>(Instant::now());
            }
        }
    })
    .await??;
    let acknowledgment = Envelope::new(
        "usage.ack",
        UsageAck {
            epoch: batch.epoch,
            seq: batch.seq,
        },
    )?;
    send_peer(&mut peer, acknowledgment.clone()).await?;
    // Incoming frames and the next real twenty-second timer survive both failed writes.
    timeout(Duration::from_secs(23), async {
        loop {
            if let Message::Heartbeat(_) = receive_peer(&mut peer).await?.0.decode()? {
                break Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await??;
    assert!((Instant::now() - first) >= Duration::from_secs(19));
    {
        let stored = runtime.state.try_lock().expect("state mutex was released");
        assert_eq!(stored.get_json::<i64>("clock_offset_ms")?, Some(0));
        assert_eq!(stored.pending_usage()?, vec![batch.clone()]);
        stored
            .connection
            .execute_batch("PRAGMA max_page_count=100000")?;
    }
    send_peer(&mut peer, acknowledgment).await?;
    peer.send(Frame::Ping(vec![9].into())).await?;
    timeout(Duration::from_secs(2), async {
        loop {
            if let Frame::Pong(bytes) = peer.next().await.context("fixture closed")?? {
                assert_eq!(bytes.as_ref(), &[9]);
                break Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await??;
    assert_eq!(runtime.state.lock().unwrap().pending_usage_count()?, 0);
    timeout(Duration::from_secs(23), async {
        loop {
            if let Message::Heartbeat(_) = receive_peer(&mut peer).await?.0.decode()? {
                break Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await??;
    assert_ne!(
        runtime
            .state
            .lock()
            .unwrap()
            .get_json::<i64>("clock_offset_ms")?,
        Some(0)
    );
    peer.send(Frame::Close(None)).await?;
    timeout(Duration::from_secs(2), tasks.join_next())
        .await?
        .context("connection missing")???;
    Ok(())
}

use super::*;

fn controlled_worker(
    directory: &Directory,
    services: Arc<Services>,
    ops: Arc<FakeResourceOps>,
) -> Result<DiagnosticWorker> {
    let config = directory.config();
    Ok(DiagnosticWorker::new(
        config.clone(),
        Arc::new(Mutex::new(State::open(&config.state_db)?)),
        vec![Arc::new(TestAdapter)],
        ops,
        services,
    )?
    .with_trusted_keys(release_support::trusted_keys()))
}

#[tokio::test]
async fn refused_memory_disk_load_conflict_and_probe_errors_never_start_a_service() -> Result<()> {
    for case in 0..9 {
        let directory = Directory::new();
        let services = Arc::new(Services::new(JobStatus::Running));
        let ops = Arc::new(FakeResourceOps::new(Arc::new(SystemOps)));
        let expected = {
            let mut probe = ops.resources.lock().unwrap();
            let resources = probe.as_mut().unwrap();
            match case {
                0 => {
                    resources.memory.host_available_bytes = 767 * 1024 * 1024;
                    "可用内存不足"
                }
                1 => {
                    resources.memory.cgroup_available_bytes = Some(767 * 1024 * 1024);
                    "可用内存不足"
                }
                2 => {
                    resources.disk_available_bytes = 2 * 1024 * 1024 * 1024 - 1;
                    "磁盘空间不足"
                }
                3 => {
                    resources.load_one = 3.01;
                    "负载过高"
                }
                4 => {
                    resources.load_one = f64::NAN;
                    "有效的 CPU 负载"
                }
                5 => {
                    resources.cpu_count = 0;
                    "有效的 CPU 负载"
                }
                6 => {
                    *services.conflicts.lock().unwrap() =
                        Ok(vec![format!("apsp-diagnostic-{}.service", Uuid::new_v4())]);
                    "同机已有诊断"
                }
                7 => {
                    *probe = Err("fixture resource read denied".into());
                    "无法读取资源状态"
                }
                8 => {
                    *services.conflicts.lock().unwrap() = Err("fixture systemd denied".into());
                    "无法检查同机诊断任务"
                }
                _ => unreachable!(),
            }
        };
        let job = super::deadline::cached_job(&directory, Uuid::new_v4())?;
        let worker = controlled_worker(&directory, services.clone(), ops)?;
        worker.save(&Checkpoint::Preparing(job.clone()))?;
        let client = PanelClient::new("http://127.0.0.1:1", "test-session")?
            .with_trusted_keys(release_support::trusted_keys());
        worker.tick(Some(&client)).await?;
        assert!(worker.active()?.is_none());
        assert_eq!(services.starts.load(Ordering::Relaxed), 0);
        let pending: Vec<DiagnosticUpdate> = worker.read(OUTBOX)?.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, job.id);
        assert_eq!(pending[0].status, DiagnosticStatus::Failed);
        assert!(
            pending[0].error.as_ref().unwrap().contains(expected),
            "case {case}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn exact_thresholds_accept_and_own_unit_does_not_conflict() -> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    let ops = Arc::new(FakeResourceOps::new(Arc::new(SystemOps)));
    {
        let mut probe = ops.resources.lock().unwrap();
        let resources = probe.as_mut().unwrap();
        resources.memory.host_available_bytes = 768 * 1024 * 1024;
        resources.disk_available_bytes = 2 * 1024 * 1024 * 1024;
        resources.load_one = 3.0;
    }
    let job = super::deadline::cached_job(&directory, Uuid::new_v4())?;
    *services.conflicts.lock().unwrap() = Ok(vec![format!("apsp-diagnostic-{}.service", job.id)]);
    let worker = controlled_worker(&directory, services.clone(), ops)?;
    worker.save(&Checkpoint::Preparing(job))?;
    let client = PanelClient::new("http://127.0.0.1:1", "test-session")?
        .with_trusted_keys(release_support::trusted_keys());
    worker.tick(Some(&client)).await?;
    assert_eq!(services.starts.load(Ordering::Relaxed), 1);
    assert!(worker.active()?.is_some());
    Ok(())
}

fn low_memory(ops: &FakeResourceOps) {
    ops.resources
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .memory
        .cgroup_available_bytes = Some(128 * 1024 * 1024 - 1);
}

#[tokio::test]
async fn state_write_failure_does_not_prevent_protection_stop_or_lose_existing_report() -> Result<()>
{
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    let ops = Arc::new(FakeResourceOps::new(Arc::new(SystemOps)));
    low_memory(&ops);
    let worker = controlled_worker(&directory, services.clone(), ops)?;
    worker.save(&checkpoint(&worker.config, Uuid::new_v4()))?;
    worker
        .state
        .lock()
        .unwrap()
        .connection
        .execute_batch("PRAGMA query_only = ON")?;
    assert!(worker.tick(None).await.is_err());
    assert!(services.stops.load(Ordering::Relaxed) > 0);
    assert!(worker.active()?.is_some());
    assert!(
        worker
            .read::<Vec<DiagnosticUpdate>>(OUTBOX)?
            .unwrap_or_default()
            .is_empty()
    );
    worker
        .state
        .lock()
        .unwrap()
        .connection
        .execute_batch("PRAGMA query_only = OFF")?;
    worker.tick(None).await?;
    let pending: Vec<DiagnosticUpdate> = worker.read(OUTBOX)?.unwrap();
    assert_eq!(pending[0].status, DiagnosticStatus::Failed);
    assert!(pending[0].report.is_some());
    assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    Ok(())
}

#[tokio::test]
async fn low_memory_and_unknown_memory_stop_offline_tasks_and_preserve_partial_reports()
-> Result<()> {
    for missing in [false, true] {
        let directory = Directory::new();
        let services = Arc::new(Services::new(JobStatus::Running));
        let ops = Arc::new(FakeResourceOps::new(Arc::new(SystemOps)));
        if missing {
            *ops.resources.lock().unwrap() = Err("fixture memory read failure".into());
        } else {
            low_memory(&ops);
        }
        let worker = controlled_worker(&directory, services.clone(), ops)?;
        let id = Uuid::new_v4();
        worker.save(&checkpoint(&worker.config, id))?;
        worker.tick(None).await?;
        assert!(worker.active()?.is_none());
        assert!(services.stops.load(Ordering::Relaxed) >= 1);
        let pending: Vec<DiagnosticUpdate> = worker.read(OUTBOX)?.unwrap();
        assert_eq!(pending[0].status, DiagnosticStatus::Failed);
        assert_eq!(pending[0].report.as_ref().unwrap().text, "fixture report");
        assert!(pending[0].error.as_ref().unwrap().contains(if missing {
            "无法确认可用内存"
        } else {
            "低内存保护"
        }));
        assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    }
    Ok(())
}

#[tokio::test]
async fn failed_or_unconfirmed_protection_stop_survives_restart_and_retries_even_if_memory_recovers()
-> Result<()> {
    for failed in [false, true] {
        let directory = Directory::new();
        let services = Arc::new(Services::new(JobStatus::Running));
        services.fail_stop.store(failed, Ordering::Relaxed);
        services.remain_active.store(!failed, Ordering::Relaxed);
        let ops = Arc::new(FakeResourceOps::new(Arc::new(SystemOps)));
        low_memory(&ops);
        {
            let first = controlled_worker(&directory, services.clone(), ops.clone())?;
            first.save(&checkpoint(&first.config, Uuid::new_v4()))?;
            assert!(first.tick(None).await.is_err());
            assert!(matches!(
                first.active()?,
                Some(Checkpoint::Started {
                    protection_stop_reason: Some(_),
                    ..
                })
            ));
            assert!(
                first
                    .read::<Vec<DiagnosticUpdate>>(OUTBOX)?
                    .unwrap_or_default()
                    .is_empty()
            );
        }
        *ops.resources.lock().unwrap() = FakeResourceOps::new(Arc::new(SystemOps))
            .resources
            .into_inner()
            .unwrap();
        services.fail_stop.store(false, Ordering::Relaxed);
        services.remain_active.store(false, Ordering::Relaxed);
        let recovered = controlled_worker(&directory, services.clone(), ops)?;
        recovered.tick(None).await?;
        assert!(recovered.active()?.is_none());
        let pending: Vec<DiagnosticUpdate> = recovered.read(OUTBOX)?.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].status, DiagnosticStatus::Failed);
        assert!(pending[0].error.as_ref().unwrap().contains("低内存保护"));
        assert!(pending[0].report.is_some());
        assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    }
    Ok(())
}

#[tokio::test]
async fn running_at_reserve_remains_running_after_worker_restart() -> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    let ops = Arc::new(FakeResourceOps::new(Arc::new(SystemOps)));
    ops.resources
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .memory
        .host_available_bytes = 128 * 1024 * 1024;
    {
        let first = controlled_worker(&directory, services.clone(), ops.clone())?;
        first.save(&checkpoint(&first.config, Uuid::new_v4()))?;
    }
    let recovered = controlled_worker(&directory, services.clone(), ops)?;
    recovered.tick(None).await?;
    assert!(recovered.active()?.is_some());
    assert_eq!(services.stops.load(Ordering::Relaxed), 0);
    assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    Ok(())
}

#[tokio::test]
async fn stalled_panel_request_does_not_delay_five_second_memory_protection() -> Result<()> {
    use tokio::{io::AsyncReadExt, net::TcpListener};

    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    let ops = Arc::new(FakeResourceOps::new(Arc::new(SystemOps)));
    let worker = Arc::new(controlled_worker(
        &directory,
        services.clone(),
        ops.clone(),
    )?);
    worker.save(&checkpoint(&worker.config, Uuid::new_v4()))?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let client = PanelClient::new(
        &format!("http://{}", listener.local_addr()?),
        "test-session",
    )?;
    let (received, request) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut bytes = [0; 8192];
        ensure!(socket.read(&mut bytes).await? > 0, "panel request is empty");
        let _ = received.send(());
        std::future::pending::<Result<()>>().await
    });
    let running = worker.clone();
    let polling = tokio::spawn(async move { running.monitored_tick(Some(&client)).await });
    tokio::time::timeout(Duration::from_secs(2), request).await??;
    low_memory(&ops);
    tokio::time::timeout(Duration::from_secs(7), async {
        while services.stops.load(Ordering::Relaxed) == 0 || worker.active()?.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await??;
    assert!(worker.active()?.is_none());
    let pending: Vec<DiagnosticUpdate> = worker.read(OUTBOX)?.unwrap();
    assert_eq!(pending[0].status, DiagnosticStatus::Failed);
    assert!(pending[0].error.as_ref().unwrap().contains("低内存保护"));
    assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    polling.abort();
    server.abort();
    Ok(())
}

#[tokio::test]
async fn low_memory_stops_despite_status_errors_and_retries_until_confirmed() -> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    services.fail_status.store(true, Ordering::Relaxed);
    let ops = Arc::new(FakeResourceOps::new(Arc::new(SystemOps)));
    low_memory(&ops);
    let first = controlled_worker(&directory, services.clone(), ops.clone())?;
    first.save(&checkpoint(&first.config, Uuid::new_v4()))?;
    assert!(first.tick(None).await.is_err());
    assert_eq!(services.stops.load(Ordering::Relaxed), 1);
    assert!(matches!(
        first.active()?,
        Some(Checkpoint::Started {
            protection_stop_reason: Some(_),
            ..
        })
    ));
    assert!(first.read::<Vec<DiagnosticUpdate>>(OUTBOX)?.is_none());
    drop(first);

    // The persisted stop is retried after restart even when pressure has cleared
    // and status remains unreadable. No terminal result is invented prematurely.
    *ops.resources.lock().unwrap() = FakeResourceOps::new(Arc::new(SystemOps))
        .resources
        .into_inner()
        .unwrap();
    let recovered = controlled_worker(&directory, services.clone(), ops)?;
    assert!(recovered.tick(None).await.is_err());
    assert_eq!(services.stops.load(Ordering::Relaxed), 2);
    assert!(recovered.active()?.is_some());
    assert!(recovered.read::<Vec<DiagnosticUpdate>>(OUTBOX)?.is_none());
    services.fail_status.store(false, Ordering::Relaxed);
    recovered.tick(None).await?;
    assert!(recovered.active()?.is_none());
    let pending: Vec<DiagnosticUpdate> = recovered.read(OUTBOX)?.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].status, DiagnosticStatus::Failed);
    assert!(pending[0].error.as_ref().unwrap().contains("低内存保护"));
    assert!(pending[0].report.is_some());
    assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    Ok(())
}

#[tokio::test]
async fn low_memory_stops_before_a_hung_status_query_times_out() -> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    services.hang.store(true, Ordering::Relaxed);
    let ops = Arc::new(FakeResourceOps::new(Arc::new(SystemOps)));
    low_memory(&ops);
    let mut worker = controlled_worker(&directory, services.clone(), ops)?;
    worker.config.operation_timeout_secs = 3;
    worker.save(&checkpoint(&worker.config, Uuid::new_v4()))?;
    // Confirmation still waits for a readable status, but stop must happen well
    // before that query's timeout. Cancelling observation leaves durable retry state.
    assert!(
        tokio::time::timeout(Duration::from_millis(500), worker.tick(None))
            .await
            .is_err()
    );
    assert_eq!(services.stops.load(Ordering::Relaxed), 1);
    assert!(matches!(
        worker.active()?,
        Some(Checkpoint::Started {
            protection_stop_reason: Some(_),
            ..
        })
    ));
    assert!(worker.read::<Vec<DiagnosticUpdate>>(OUTBOX)?.is_none());
    services.hang.store(false, Ordering::Relaxed);
    worker.tick(None).await?;
    let pending: Vec<DiagnosticUpdate> = worker.read(OUTBOX)?.unwrap();
    assert_eq!(pending[0].status, DiagnosticStatus::Failed);
    assert!(pending[0].error.as_ref().unwrap().contains("低内存保护"));
    assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    Ok(())
}

#[tokio::test]
async fn memory_pressure_is_resampled_while_a_status_query_is_hung() -> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    services.hang.store(true, Ordering::Relaxed);
    let ops = Arc::new(FakeResourceOps::new(Arc::new(SystemOps)));
    let mut worker = controlled_worker(&directory, services.clone(), ops.clone())?;
    worker.config.operation_timeout_secs = 15;
    worker.save(&checkpoint(&worker.config, Uuid::new_v4()))?;
    let worker = Arc::new(worker);
    let running = worker.clone();
    let polling = tokio::spawn(async move { running.tick(None).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(services.stops.load(Ordering::Relaxed), 0);
    low_memory(&ops);
    tokio::time::timeout(Duration::from_secs(7), async {
        while services.stops.load(Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    assert!(matches!(
        worker.active()?,
        Some(Checkpoint::Started {
            protection_stop_reason: Some(_),
            ..
        })
    ));
    assert!(worker.read::<Vec<DiagnosticUpdate>>(OUTBOX)?.is_none());
    assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    polling.abort();
    Ok(())
}

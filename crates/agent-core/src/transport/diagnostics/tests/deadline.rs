use super::*;

struct DelayedAdapter {
    delay: Duration,
    prepared_limits: Arc<Mutex<Vec<u32>>>,
}

impl DiagnosticAdapter for DelayedAdapter {
    fn describe(&self) -> DiagnosticDescriptor {
        TestAdapter.describe()
    }

    fn prepare<'a>(
        &'a self,
        spec: &'a DiagnosticSpec,
        privileged: &'a dyn Privileged,
    ) -> BoxFuture<'a, ServiceJob> {
        Box::pin(async move {
            self.prepared_limits.lock().unwrap().push(spec.timeout_secs);
            tokio::time::sleep(self.delay).await;
            TestAdapter.prepare(spec, privileged).await
        })
    }

    fn collect<'a>(&'a self, spec: &'a DiagnosticSpec) -> BoxFuture<'a, Option<DiagnosticOutput>> {
        TestAdapter.collect(spec)
    }
}

pub(super) fn cached_job(directory: &Directory, id: Uuid) -> Result<DiagnosticJob> {
    let mut job = job(id);
    job.artifact.url = format!(
        "http://127.0.0.1:1/api/agent/v1/artifacts/diagnostic-fixture/v1/{}",
        apsp_protocol::release::native_arch()?
    );
    let artifact = directory
        .config()
        .install_root
        .join("diagnostic-fixture/v1");
    std::fs::create_dir_all(&artifact)?;
    let binary = b"fixture diagnostic binary";
    std::fs::write(artifact.join("runner"), binary)?;
    let proof = release_support::proof_for_archive(
        "diagnostic-fixture",
        "v1",
        "runner",
        b"cached archive",
        binary,
    );
    job.artifact.sha256 = release_support::hash(b"cached archive");
    job.artifact.proof = Some(proof.clone());
    release_support::install_proof(&artifact, &proof);
    Ok(job)
}

fn delayed_worker(
    directory: &Directory,
    services: Arc<Services>,
    delay: Duration,
    prepared_limits: Arc<Mutex<Vec<u32>>>,
) -> Result<DiagnosticWorker> {
    let config = directory.config();
    Ok(DiagnosticWorker::new(
        config.clone(),
        Arc::new(Mutex::new(State::open(&config.state_db)?)),
        vec![Arc::new(DelayedAdapter {
            delay,
            prepared_limits,
        })],
        Arc::new(FakeResourceOps::new(Arc::new(SystemOps))),
        services,
    )?
    .with_trusted_keys(release_support::trusted_keys()))
}

#[test]
fn effective_service_limit_respects_configured_and_absolute_deadlines() -> Result<()> {
    assert_eq!(execution_timeout(300, Some(1050), 1000)?, 50);
    assert_eq!(execution_timeout(300, Some(1400), 1000)?, 300);
    assert_eq!(execution_timeout(300, None, 1000)?, 300);
    for deadline in [1000, 999, -1] {
        assert!(execution_timeout(300, Some(deadline), 1000).is_err());
    }
    Ok(())
}

#[tokio::test]
async fn resumed_preparation_recomputes_remaining_runtime_after_adapter_delay() -> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    let prepared_limits = Arc::new(Mutex::new(Vec::new()));
    let mut job = cached_job(&directory, Uuid::new_v4())?;
    let deadline = unix_time() as i64 + 30;
    job.expires_at = Some(deadline);
    {
        let first = worker(&directory, services.clone())?;
        first.save(&Checkpoint::Preparing(job.clone()))?;
    }
    let recovered = delayed_worker(
        &directory,
        services.clone(),
        Duration::from_millis(1100),
        prepared_limits.clone(),
    )?;
    let client = PanelClient::new("http://127.0.0.1:1", "test-session")?
        .with_trusted_keys(release_support::trusted_keys());
    recovered.tick(Some(&client)).await?;
    let Some(Checkpoint::Started {
        spec,
        service,
        started_at,
        expires_at,
        ..
    }) = recovered.active()?
    else {
        anyhow::bail!("resumed diagnostic was not started");
    };
    assert_eq!(expires_at, Some(deadline));
    assert_eq!(
        service.timeout_secs,
        u32::try_from(deadline as u64 - started_at)?
    );
    assert_eq!(spec.timeout_secs, service.timeout_secs);
    assert!(service.timeout_secs < prepared_limits.lock().unwrap()[0]);
    assert_eq!(
        services
            .last_job
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .timeout_secs,
        service.timeout_secs
    );
    assert_eq!(services.starts.load(Ordering::Relaxed), 1);
    Ok(())
}

#[tokio::test]
async fn deadline_crossed_during_preparation_never_starts_the_service() -> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    let mut job = cached_job(&directory, Uuid::new_v4())?;
    job.expires_at = Some(unix_time() as i64 + 2);
    let worker = delayed_worker(
        &directory,
        services.clone(),
        Duration::from_millis(2100),
        Arc::new(Mutex::new(Vec::new())),
    )?;
    worker.save(&Checkpoint::Preparing(job.clone()))?;
    let client = PanelClient::new("http://127.0.0.1:1", "test-session")?
        .with_trusted_keys(release_support::trusted_keys());
    worker.tick(Some(&client)).await?;
    assert!(worker.active()?.is_none());
    let pending: Vec<DiagnosticUpdate> = worker.read(OUTBOX)?.unwrap();
    assert_eq!(pending[0].status, DiagnosticStatus::Failed);
    assert!(pending[0].error.as_ref().unwrap().contains("expired"));
    assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    Ok(())
}

#[tokio::test]
async fn recovered_running_service_is_stopped_at_its_saved_absolute_deadline() -> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    {
        let first = worker(&directory, services.clone())?;
        let mut saved = checkpoint(&first.config, Uuid::new_v4());
        if let Checkpoint::Started { expires_at, .. } = &mut saved {
            *expires_at = Some(unix_time() as i64 - 1);
        }
        first.save(&saved)?;
    }
    let recovered = worker(&directory, services.clone())?;
    recovered.tick(None).await?;
    assert!(recovered.active()?.is_none());
    let pending: Vec<DiagnosticUpdate> = recovered.read(OUTBOX)?.unwrap();
    assert_eq!(pending[0].status, DiagnosticStatus::Failed);
    assert!(
        pending[0]
            .error
            .as_ref()
            .unwrap()
            .contains("absolute deadline")
    );
    assert!(services.stops.load(Ordering::Relaxed) >= 1);
    assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    Ok(())
}

#[tokio::test]
async fn completed_service_keeps_its_late_report_after_the_absolute_deadline() -> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Succeeded));
    let worker = worker(&directory, services)?;
    let mut saved = checkpoint(&worker.config, Uuid::new_v4());
    if let Checkpoint::Started { expires_at, .. } = &mut saved {
        *expires_at = Some(unix_time() as i64 - 1);
    }
    worker.save(&saved)?;
    worker.tick(None).await?;
    let pending: Vec<DiagnosticUpdate> = worker.read(OUTBOX)?.unwrap();
    assert_eq!(pending[0].status, DiagnosticStatus::Succeeded);
    assert_eq!(pending[0].report.as_ref().unwrap().text, "fixture report");
    Ok(())
}

struct TamperingAdapter;
impl DiagnosticAdapter for TamperingAdapter {
    fn describe(&self) -> DiagnosticDescriptor {
        TestAdapter.describe()
    }
    fn prepare<'a>(
        &'a self,
        spec: &'a DiagnosticSpec,
        privileged: &'a dyn Privileged,
    ) -> BoxFuture<'a, ServiceJob> {
        Box::pin(async move {
            let service = TestAdapter.prepare(spec, privileged).await?;
            std::fs::write(&spec.binary_path, b"tampered diagnostic binary")?;
            Ok(service)
        })
    }
    fn collect<'a>(&'a self, spec: &'a DiagnosticSpec) -> BoxFuture<'a, Option<DiagnosticOutput>> {
        TestAdapter.collect(spec)
    }
}

#[tokio::test]
async fn unsigned_resumed_jobs_are_failed_without_executing_a_service() -> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    let mut job = cached_job(&directory, Uuid::new_v4())?;
    job.artifact.proof = None;
    let worker = worker(&directory, services.clone())?;
    worker.save(&Checkpoint::Preparing(job))?;
    let client = PanelClient::new("http://127.0.0.1:1", "test-session")?
        .with_trusted_keys(release_support::trusted_keys());
    worker.tick(Some(&client)).await?;
    assert!(worker.active()?.is_none());
    assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    let pending: Vec<DiagnosticUpdate> = worker.read(OUTBOX)?.unwrap();
    assert_eq!(pending[0].status, DiagnosticStatus::Failed);
    assert!(
        pending[0]
            .error
            .as_ref()
            .unwrap()
            .contains("signed release proof")
    );
    Ok(())
}

#[tokio::test]
async fn binary_changed_during_preparation_is_rechecked_before_systemd_start() -> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    let job = cached_job(&directory, Uuid::new_v4())?;
    let config = directory.config();
    let worker = DiagnosticWorker::new(
        config.clone(),
        Arc::new(Mutex::new(State::open(&config.state_db)?)),
        vec![Arc::new(TamperingAdapter)],
        Arc::new(FakeResourceOps::new(Arc::new(SystemOps))),
        services.clone(),
    )?
    .with_trusted_keys(release_support::trusted_keys());
    worker.save(&Checkpoint::Preparing(job))?;
    let client = PanelClient::new("http://127.0.0.1:1", "test-session")?
        .with_trusted_keys(release_support::trusted_keys());
    worker.tick(Some(&client)).await?;
    assert!(worker.active()?.is_none());
    assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    let pending: Vec<DiagnosticUpdate> = worker.read(OUTBOX)?.unwrap();
    assert_eq!(pending[0].status, DiagnosticStatus::Failed);
    assert!(pending[0].error.as_ref().unwrap().contains("signed size"));
    Ok(())
}

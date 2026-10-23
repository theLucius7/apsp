use super::*;

struct ChapterAdapter;
static FALLBACK_ADAPTER: TestAdapter = TestAdapter;
impl DiagnosticAdapter for ChapterAdapter {
    fn describe(&self) -> DiagnosticDescriptor {
        FALLBACK_ADAPTER.describe()
    }
    fn prepare<'a>(
        &'a self,
        spec: &'a DiagnosticSpec,
        privileged: &'a dyn Privileged,
    ) -> BoxFuture<'a, ServiceJob> {
        FALLBACK_ADAPTER.prepare(spec, privileged)
    }
    fn collect<'a>(&'a self, spec: &'a DiagnosticSpec) -> BoxFuture<'a, Option<DiagnosticOutput>> {
        FALLBACK_ADAPTER.collect(spec)
    }
    fn collect_sections<'a>(
        &'a self,
        spec: &'a DiagnosticSpec,
    ) -> BoxFuture<'a, Vec<apsp_adapter_sdk::DiagnosticSection>> {
        Box::pin(async move {
            match tokio::fs::read(spec.job_dir.join("fixture-sections.json")).await {
                Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
                Err(error) => Err(error.into()),
            }
        })
    }
}

fn chapter_worker(directory: &Directory, services: Arc<Services>) -> Result<DiagnosticWorker> {
    let config = directory.config();
    Ok(DiagnosticWorker::new(
        config.clone(),
        Arc::new(Mutex::new(State::open(&config.state_db)?)),
        vec![Arc::new(ChapterAdapter)],
        Arc::new(SystemOps),
        services,
    )?
    .with_trusted_keys(release_support::trusted_keys()))
}

#[tokio::test]
async fn partial_chapters_survive_a_worker_restart_and_failed_service_without_reexecution()
-> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    let id = Uuid::new_v4();
    let first = chapter_worker(&directory, services.clone())?;
    let saved = checkpoint(&first.config, id);
    let Checkpoint::Started { spec, .. } = &saved else {
        unreachable!()
    };
    std::fs::create_dir_all(&spec.job_dir)?;
    let chapters = serde_json::json!([
        {"name":"header_info","text":"durable header","complete":true,"revision":1,"collected_at":1700000000},
        {"name":"hardware_quality","text":"partial hardware","complete":false,"revision":1,"collected_at":1700000000}
    ]);
    std::fs::write(
        spec.job_dir.join("fixture-sections.json"),
        serde_json::to_vec(&chapters)?,
    )?;
    first.save(&saved)?;
    first.tick(None).await?;
    let pending: Vec<apsp_protocol::DiagnosticSectionUpdate> =
        first.read(sections::SECTIONS_OUTBOX)?.unwrap();
    assert_eq!(pending.len(), 2);
    drop(first);
    *services.status.lock().unwrap() = JobStatus::Failed {
        error: "OOM fixture".into(),
    };
    let recovered = chapter_worker(&directory, services.clone())?;
    recovered.tick(None).await?;
    assert!(recovered.active()?.is_none());
    let pending: Vec<apsp_protocol::DiagnosticSectionUpdate> =
        recovered.read(sections::SECTIONS_OUTBOX)?.unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].text, "durable header");
    let terminal: Vec<DiagnosticUpdate> = recovered.read(OUTBOX)?.unwrap();
    assert_eq!(terminal[0].status, DiagnosticStatus::Failed);
    assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    Ok(())
}

#[tokio::test]
async fn chapter_http_failure_does_not_hide_terminal_status_and_acknowledgment_is_durable()
-> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let reject_chapters = Arc::new(AtomicBool::new(true));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let rejected = reject_chapters.clone();
    let received = requests.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let header_end = loop {
                let mut block = [0; 4096];
                let count = socket.read(&mut block).await.unwrap();
                if count == 0 {
                    return;
                }
                request.extend_from_slice(&block[..count]);
                if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    break end + 4;
                }
                assert!(request.len() <= 16384);
            };
            let headers = String::from_utf8_lossy(&request[..header_end]).into_owned();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse().unwrap())
                })
                .unwrap_or(0);
            assert!(length <= 512 * 1024);
            while request.len() < header_end + length {
                let mut block = [0; 4096];
                let count = socket.read(&mut block).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&block[..count]);
            }
            let is_chapter = headers.lines().next().unwrap().contains("/sections ");
            received.lock().unwrap().push(is_chapter);
            let status = if is_chapter && rejected.load(Ordering::Relaxed) {
                "503 Service Unavailable"
            } else {
                "204 No Content"
            };
            socket
                .write_all(
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Failed {
        error: "fixture stopped".into(),
    }));
    let first = chapter_worker(&directory, services.clone())?;
    let id = Uuid::new_v4();
    let chapter = apsp_protocol::DiagnosticSectionUpdate {
        id,
        name: "header_info".into(),
        text: "saved during disconnect".into(),
        complete: true,
        revision: 1,
        collected_at: 1700000000,
    };
    first
        .state
        .lock()
        .unwrap()
        .set_json(sections::SECTIONS_OUTBOX, &vec![chapter.clone()])?;
    first.finish(failure(id, "fixture stopped".into(), None))?;
    let client = PanelClient::new(&format!("http://{address}"), "fixture-session")?;
    assert!(first.flush(&client).await.is_err());
    assert!(
        first
            .read::<Vec<DiagnosticUpdate>>(OUTBOX)?
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        first
            .read::<Vec<apsp_protocol::DiagnosticSectionUpdate>>(sections::SECTIONS_OUTBOX)?
            .unwrap(),
        vec![chapter.clone()]
    );
    assert_eq!(*requests.lock().unwrap(), vec![true, false]);
    drop(first);
    reject_chapters.store(false, Ordering::Relaxed);
    let recovered = chapter_worker(&directory, services)?;
    recovered.flush(&client).await?;
    assert!(
        recovered
            .read::<Vec<apsp_protocol::DiagnosticSectionUpdate>>(sections::SECTIONS_OUTBOX)?
            .unwrap()
            .is_empty()
    );
    let sent: BTreeMap<String, u64> = recovered
        .read(&format!("diagnostics:sections:sent:{id}"))?
        .unwrap();
    assert_eq!(sent.get("header_info"), Some(&1));
    server.abort();
    Ok(())
}

#[tokio::test]
async fn confirmed_cancellation_captures_final_chapters_and_keeps_them_after_restart() -> Result<()>
{
    use crate::transport::diagnostics::cancellation::CancellationControl;
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    let id = Uuid::new_v4();
    let worker = chapter_worker(&directory, services.clone())?;
    let control = Arc::new(CancellationControl::new(
        worker.state.clone(),
        7,
        vec!["diagnostic-fixture".into()],
    ));
    let worker = worker.with_cancellations(control.clone());
    let checkpoint = checkpoint(&worker.config, id);
    let Checkpoint::Started { spec, .. } = &checkpoint else {
        unreachable!()
    };
    std::fs::create_dir_all(&spec.job_dir)?;
    std::fs::write(
        spec.job_dir.join("fixture-sections.json"),
        serde_json::to_vec(&serde_json::json!([
            {"name":"header_info","text":"final cancelled chapter","complete":true,"revision":2,"collected_at":1700000000}
        ]))?,
    )?;
    worker.save(&checkpoint)?;
    control.request(apsp_protocol::DiagnosticCancelRequest {
        server_id: 7,
        job: job(id),
    })?;
    worker.tick(None).await?;
    assert!(worker.active()?.is_none());
    let results: Vec<apsp_protocol::DiagnosticCancelResult> =
        worker.read("diagnostics:cancellation-results")?.unwrap();
    assert!(results[0].confirmed);
    drop(worker);
    drop(control);
    let recovered = chapter_worker(&directory, services.clone())?;
    let chapters: Vec<apsp_protocol::DiagnosticSectionUpdate> =
        recovered.read(sections::SECTIONS_OUTBOX)?.unwrap();
    assert_eq!(chapters.len(), 1);
    assert_eq!(chapters[0].text, "final cancelled chapter");
    assert_eq!(chapters[0].revision, 2);
    recovered.accept(vec![job(id)])?;
    assert!(recovered.active()?.is_none());
    assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    Ok(())
}

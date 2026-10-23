use super::*;
use crate::transport::diagnostics::cancellation::CancellationControl;
use apsp_protocol::{DiagnosticCancelRequest, DiagnosticCancelResult};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const RESULTS: &str = "diagnostics:cancellation-results";
const REQUESTS: &str = "diagnostics:cancellations";

fn controlled(
    directory: &Directory,
    services: Arc<Services>,
) -> Result<(DiagnosticWorker, Arc<CancellationControl>)> {
    let worker = worker(directory, services)?;
    let control = Arc::new(CancellationControl::new(
        worker.state.clone(),
        7,
        vec!["diagnostic-fixture".into()],
    ));
    Ok((worker.with_cancellations(control.clone()), control))
}
fn request(id: Uuid) -> DiagnosticCancelRequest {
    DiagnosticCancelRequest {
        server_id: 7,
        job: job(id),
    }
}

#[tokio::test]
async fn preparing_cancellation_is_durable_idempotent_and_never_starts_later() -> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Missing));
    let id = Uuid::new_v4();
    {
        let (first, control) = controlled(&directory, services.clone())?;
        first.save(&Checkpoint::Preparing(job(id)))?;
        control.request(request(id))?;
        control.request(request(id))?;
        let pending: Vec<DiagnosticCancelRequest> = first.read(REQUESTS)?.unwrap();
        assert_eq!(pending.len(), 1);
    }
    let (recovered, _) = controlled(&directory, services.clone())?;
    recovered.tick(None).await?;
    let results: Vec<DiagnosticCancelResult> = recovered.read(RESULTS)?.unwrap();
    assert!(results[0].confirmed);
    assert!(recovered.active()?.is_none());
    recovered.accept(vec![job(id)])?;
    assert!(recovered.active()?.is_none());
    assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    assert_eq!(services.stops.load(Ordering::Relaxed), 0);
    Ok(())
}

#[tokio::test]
async fn stop_failure_live_processes_and_remaining_mounts_require_confirmation_after_restart()
-> Result<()> {
    for case in 0..3 {
        let directory = Directory::new();
        let services = Arc::new(Services::new(JobStatus::Running));
        services.fail_stop.store(case == 0, Ordering::Relaxed);
        services.remain_active.store(case == 1, Ordering::Relaxed);
        services
            .cleanup_confirmed
            .store(case != 2, Ordering::Relaxed);
        let id = Uuid::new_v4();
        {
            let (first, control) = controlled(&directory, services.clone())?;
            first.save(&checkpoint(&first.config, id))?;
            control.request(request(id))?;
            first.tick(None).await?;
            assert!(first.active()?.is_some());
            let results: Vec<DiagnosticCancelResult> = first.read(RESULTS)?.unwrap();
            assert!(!results[0].confirmed);
            assert!(results[0].error.as_ref().unwrap().contains("取消尚未确认"));
            assert!(
                !first
                    .read::<bool>(&format!("diagnostics:done:{id}"))?
                    .unwrap_or(false)
            );
        }
        services.fail_stop.store(false, Ordering::Relaxed);
        services.remain_active.store(false, Ordering::Relaxed);
        services.cleanup_confirmed.store(true, Ordering::Relaxed);
        let (recovered, _) = controlled(&directory, services.clone())?;
        recovered.tick(None).await?;
        let results: Vec<DiagnosticCancelResult> = recovered.read(RESULTS)?.unwrap();
        assert!(results[0].confirmed);
        assert_eq!(results[0].report.as_ref().unwrap().text, "fixture report");
        assert!(recovered.active()?.is_none());
        assert_eq!(services.starts.load(Ordering::Relaxed), 0);
        assert!(
            services
                .stopped_units
                .lock()
                .unwrap()
                .iter()
                .all(|unit| unit == &format!("apsp-diagnostic-{id}.service"))
        );
    }
    Ok(())
}

#[tokio::test]
async fn foreign_device_plugin_and_corrupt_saved_unit_cannot_cancel_an_arbitrary_service()
-> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    let (worker, control) = controlled(&directory, services.clone())?;
    let id = Uuid::new_v4();
    let mut foreign = request(id);
    foreign.server_id = 8;
    assert!(control.request(foreign).is_err());
    let mut foreign = request(id);
    foreign.job.plugin = "not-registered".into();
    assert!(control.request(foreign).is_err());
    for version in ["../outside", "/outside", ""] {
        let mut unsafe_target = request(id);
        unsafe_target.job.version = version.into();
        assert!(control.request(unsafe_target).is_err());
    }
    assert!(control.request(request(Uuid::nil())).is_err());
    assert!(
        worker
            .read::<Vec<DiagnosticCancelRequest>>(REQUESTS)?
            .is_none()
    );
    let mut saved = checkpoint(&worker.config, id);
    if let Checkpoint::Started { service, .. } = &mut saved {
        service.unit = "sshd.service".into();
    }
    worker.save(&saved)?;
    control.request(request(id))?;
    let error = worker.tick(None).await.unwrap_err();
    assert!(error.to_string().contains("owned task identity"));
    assert_eq!(services.stops.load(Ordering::Relaxed), 0);
    let results: Vec<DiagnosticCancelResult> = worker.read(RESULTS)?.unwrap();
    assert!(!results[0].confirmed);
    let mut wire = serde_json::to_value(request(id))?;
    wire["unit"] = serde_json::json!("sshd.service");
    assert!(serde_json::from_value::<DiagnosticCancelRequest>(wire).is_err());
    Ok(())
}

#[tokio::test]
async fn cancelling_a_stalled_signed_artifact_download_interrupts_it_without_starting() -> Result<()>
{
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Missing));
    let (worker, control) = controlled(&directory, services.clone())?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let mut job = super::deadline::cached_job(&directory, Uuid::new_v4())?;
    job.artifact.url = job.artifact.url.replacen("http://127.0.0.1:1", &base, 1);
    std::fs::remove_dir_all(worker.config.install_root.join("diagnostic-fixture"))?;
    worker.save(&Checkpoint::Preparing(job.clone()))?;
    let (received, started) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut bytes = [0; 8192];
        ensure!(
            socket.read(&mut bytes).await? > 0,
            "download request is empty"
        );
        let _ = received.send(());
        std::future::pending::<Result<()>>().await
    });
    let client =
        PanelClient::new(&base, "test-session")?.with_trusted_keys(release_support::trusted_keys());
    let polling = tokio::spawn(async move {
        worker.monitored_tick(Some(&client)).await?;
        Ok::<_, anyhow::Error>(worker)
    });
    tokio::time::timeout(Duration::from_secs(2), started).await??;
    control.request(DiagnosticCancelRequest { server_id: 7, job })?;
    let worker = tokio::time::timeout(Duration::from_secs(2), polling).await???;
    assert!(worker.active()?.is_none());
    assert_eq!(services.starts.load(Ordering::Relaxed), 0);
    let results: Vec<DiagnosticCancelResult> = worker.read(RESULTS)?.unwrap();
    assert!(results[0].confirmed);
    server.abort();
    Ok(())
}

#[tokio::test]
async fn cancellation_result_survives_http_failure_and_replays_after_sqlite_reopen() -> Result<()> {
    let directory = Directory::new();
    let services = Arc::new(Services::new(JobStatus::Running));
    let id = Uuid::new_v4();
    let (worker, control) = controlled(&directory, services.clone())?;
    worker.save(&checkpoint(&worker.config, id))?;
    control.request(request(id))?;
    worker.tick(None).await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let client = PanelClient::new(
        &format!("http://{}", listener.local_addr()?),
        "test-session",
    )?;
    let server = tokio::spawn(async move {
        for status in ["500 Internal Server Error", "204 No Content"] {
            let (mut socket, _) = listener.accept().await?;
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 8192];
                let count = socket.read(&mut chunk).await?;
                ensure!(count > 0, "confirmation closed early");
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(end) = bytes.windows(4).position(|chunk| chunk == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&bytes[..end])?;
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse())
                        })
                        .transpose()?
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        assert!(headers.starts_with(&format!(
                            "POST /api/agent/v1/diagnostics/{id}/cancel-confirmation "
                        )));
                        let result: DiagnosticCancelResult =
                            serde_json::from_slice(&bytes[end + 4..])?;
                        assert!(result.confirmed && result.id == id && result.server_id == 7);
                        break;
                    }
                }
            }
            socket
                .write_all(
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                )
                .await?;
        }
        Ok::<_, anyhow::Error>(())
    });
    assert!(control.flush(&client).await.is_err());
    drop(worker);
    drop(control);
    let (recovered, control) = controlled(&directory, services)?;
    let results: Vec<DiagnosticCancelResult> = recovered.read(RESULTS)?.unwrap();
    assert_eq!(results.len(), 1);
    control.flush(&client).await?;
    let results: Vec<DiagnosticCancelResult> = recovered.read(RESULTS)?.unwrap();
    assert!(results.is_empty());
    server.await??;
    Ok(())
}

#[tokio::test]
async fn retirement_during_cancellation_http_cannot_restore_cleared_state() -> Result<()> {
    for acknowledging in [false, true] {
        let directory = Directory::new();
        let services = Arc::new(Services::new(JobStatus::Missing));
        let (worker, control) = controlled(&directory, services.clone())?;
        let id = Uuid::new_v4();
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let client = Arc::new(PanelClient::new(
            &format!("http://{}", listener.local_addr()?),
            "test-session",
        )?);
        if acknowledging {
            worker.state.lock().unwrap().set_json(
                RESULTS,
                &vec![DiagnosticCancelResult {
                    server_id: 7,
                    id,
                    plugin: "diagnostic-fixture".into(),
                    confirmed: true,
                    report: None,
                    error: None,
                }],
            )?;
        }
        let retirement = Arc::new(crate::retirement::Retirement::new(
            worker.config.clone(),
            worker.state.clone(),
            vec![],
            Arc::new(FakeResourceOps::new(Arc::new(SystemOps))),
            services,
        )?);
        let body = if acknowledging {
            Vec::new()
        } else {
            serde_json::to_vec(&vec![request(id)])?
        };
        let (seen, started) = tokio::sync::oneshot::channel();
        let (reply, released) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 8192];
                let count = socket.read(&mut chunk).await?;
                ensure!(count > 0, "cancellation HTTP request closed early");
                bytes.extend_from_slice(&chunk[..count]);
                if bytes.windows(4).any(|chunk| chunk == b"\r\n\r\n") {
                    break;
                }
            }
            let headers = std::str::from_utf8(&bytes)?;
            assert!(headers.starts_with(if acknowledging { "POST " } else { "GET " }));
            seen.send(())
                .map_err(|_| anyhow::anyhow!("test receiver closed"))?;
            released.await?;
            socket.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()
            ).as_bytes()).await?;
            socket.write_all(&body).await?;
            Ok::<_, anyhow::Error>(())
        });
        let (clients, receiving) = watch::channel(Some(client));
        let polling = tokio::spawn(control.run(receiving, retirement.clone()));
        tokio::time::timeout(Duration::from_secs(2), started).await??;
        retirement.request(
            &crate::identity::Identity {
                server_id: 7,
                signing_key: ed25519_dalek::SigningKey::from_bytes(&[17; 32]),
            },
            apsp_protocol::RetirementRequest {
                request_id: Uuid::new_v4(),
            },
        )?;
        let retiring = retirement.clone();
        let state = worker.state.clone();
        let clearing = tokio::spawn(async move {
            let _guard = retiring.gate.write().await;
            state.lock().unwrap().clear_retired_configuration()
        });
        // Retirement's destructive phase waits for the already in-flight request.
        tokio::task::yield_now().await;
        assert!(!clearing.is_finished());
        reply
            .send(())
            .map_err(|_| anyhow::anyhow!("HTTP fixture closed"))?;
        tokio::time::timeout(Duration::from_secs(2), server).await???;
        tokio::time::timeout(Duration::from_secs(2), clearing).await???;
        // Another recovery wake after clearing must also leave the keys absent.
        clients.send_replace(None);
        drop(clients);
        tokio::time::timeout(Duration::from_secs(2), polling).await???;
        assert!(worker.read::<serde_json::Value>(REQUESTS)?.is_none());
        assert!(worker.read::<serde_json::Value>(RESULTS)?.is_none());
    }
    Ok(())
}

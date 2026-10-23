use super::*;
use crate::{
    Config, State,
    fake::{FakeAdapter, FakeServiceManager},
    system::SystemOps,
};
use std::sync::{Arc, Mutex, atomic::Ordering};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn fixture(
    request: RuntimeOperationRequest,
    drop_first: bool,
) -> (
    PanelClient,
    tokio::task::JoinHandle<Vec<RuntimeOperationResult>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let mut results = Vec::new();
        let attempts = if drop_first { 2 } else { 1 };
        for _ in 0..attempts * 2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let (start, length) = loop {
                let mut chunk = [0; 4096];
                let count = stream.read(&mut chunk).await.unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(at) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&bytes[..at]);
                    let length: usize = header
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= at + 4 + length {
                        break (at + 4, length);
                    }
                }
            };
            let body = if bytes.starts_with(b"GET ") {
                if results.is_empty() {
                    serde_json::to_vec(&vec![request.clone()]).unwrap()
                } else {
                    // A committed result disappears from the pending queue even
                    // if its HTTP acknowledgment never reached the device.
                    b"[]".to_vec()
                }
            } else {
                let completed: RuntimeOperationResult =
                    serde_json::from_slice(&bytes[start..start + length]).unwrap();
                assert!(completed.valid());
                results.push(completed);
                if drop_first && results.len() == 1 {
                    continue;
                }
                serde_json::to_vec(&TaskAck {
                    ids: vec![request.id],
                })
                .unwrap()
            };
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        }
        results
    });
    (
        PanelClient::new(&format!("http://{address}"), "fixture-token").unwrap(),
        handle,
    )
}

fn request() -> RuntimeOperationRequest {
    RuntimeOperationRequest {
        id: uuid::Uuid::new_v4(),
        module: "demo".into(),
        operation: RuntimeOperation::Inspect,
        expected_revision: None,
        requested_at: now_timestamp(),
        expires_at: now_timestamp() + 600,
    }
}

#[tokio::test]
async fn result_delivery_failure_reopens_durable_result_without_reexecuting() {
    let path = std::env::temp_dir().join(format!("apsp-runtime-{}.db", uuid::Uuid::new_v4()));
    let state = Arc::new(Mutex::new(State::open(&path).unwrap()));
    let services = Arc::new(FakeServiceManager::default());
    let reconciler = Reconciler::new(
        Config::default(),
        state.clone(),
        Arc::new(FakeAdapter::default()),
        Arc::new(SystemOps),
        services.clone(),
    );
    let reconcilers = vec![("demo".into(), Arc::new(reconciler))];
    let (outgoing, _receiver) = tokio::sync::mpsc::channel(8);
    let (client, fixture) = fixture(request(), true).await;
    assert!(
        poll(&reconcilers, &state, &client, &outgoing)
            .await
            .is_err()
    );
    services.active.store(true, Ordering::SeqCst);
    let reopened = Arc::new(Mutex::new(State::open(&path).unwrap()));
    poll(&reconcilers, &reopened, &client, &outgoing)
        .await
        .unwrap();
    let results = fixture.await.unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0], results[1]);
    assert_eq!(
        results[0].snapshot.as_ref().unwrap().service,
        apsp_protocol::RuntimeServiceState::Inactive
    );
    assert!(records(&reopened).unwrap()[0].acknowledged);
    drop(reopened);
    drop(reconcilers);
    drop(state);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn interrupted_operation_is_reported_without_reexecution() {
    let state = Arc::new(Mutex::new(
        State::open(std::path::Path::new(":memory:")).unwrap(),
    ));
    let request = request();
    save(
        &state,
        &[Record {
            request: request.clone(),
            result: None,
            acknowledged: false,
        }],
    )
    .unwrap();
    let (client, fixture) = fixture(request, false).await;
    let (outgoing, _receiver) = tokio::sync::mpsc::channel(8);
    poll(&[], &state, &client, &outgoing).await.unwrap();
    assert_eq!(
        fixture.await.unwrap()[0].error,
        Some(RuntimeOperationError::Interrupted)
    );
}

#[tokio::test]
async fn clock_rounding_cannot_put_observation_or_completion_before_the_request() {
    let state = Arc::new(Mutex::new(
        State::open(std::path::Path::new(":memory:")).unwrap(),
    ));
    let reconciler = Reconciler::new(
        Config::default(),
        state,
        Arc::new(FakeAdapter::default()),
        Arc::new(SystemOps),
        Arc::new(FakeServiceManager::default()),
    );
    let mut request = request();
    request.requested_at += 30;
    request.expires_at += 30;
    let failure = result(&request, Some(RuntimeOperationError::Interrupted), -60);
    assert_eq!(failure.finished_at, request.requested_at);
    assert!(failure.valid());
    let client = PanelClient::new("http://127.0.0.1:1", "fixture-token").unwrap();
    let (outgoing, _receiver) = tokio::sync::mpsc::channel(8);
    let completed = execute(
        &request,
        &[("demo".into(), Arc::new(reconciler))],
        &client,
        &outgoing,
        0,
    )
    .await;
    assert!(completed.valid());
    assert!(completed.error.is_none());
    assert_eq!(
        completed.snapshot.as_ref().unwrap().observed_at,
        request.requested_at
    );
    assert!(completed.finished_at >= request.requested_at);
}

use super::*;
use crate::State;
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[tokio::test]
async fn committed_barrier_result_is_replayed_after_expiry_and_database_reopen() {
    let path = std::env::temp_dir().join(format!("apsp-validation-{}.db", uuid::Uuid::new_v4()));
    let state = Arc::new(Mutex::new(State::open(&path).unwrap()));
    let request = RuntimeValidationRequest {
        id: uuid::Uuid::new_v4(),
        module: "demo".into(),
        scope: "scope:1".into(),
        generation: 2,
        operation: apsp_protocol::RuntimeValidationOperation::Barrier,
        revision: 3,
        config_hash: "a".repeat(64),
        expires_at: now_timestamp() - 1,
    };
    let completed = RuntimeValidationResult {
        request: request.clone(),
        success: true,
        error: None,
        checked_at: request.expires_at - 1,
    };
    save(
        &state,
        &[Record {
            request: request.clone(),
            result: Some(completed.clone()),
            acknowledged: false,
        }],
    )
    .unwrap();
    state
        .lock()
        .unwrap()
        .set_json(
            "runtime_generation_floor:demo",
            &std::collections::BTreeMap::from([("scope:1", 2u64)]),
        )
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = PanelClient::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        "fixture-token",
    )
    .unwrap();
    let fixture = tokio::spawn(async move {
        let mut results = Vec::new();
        for index in 0..3 {
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
                        .unwrap_or_default();
                    if bytes.len() >= at + 4 + length {
                        break (at + 4, length);
                    }
                }
            };
            let body = if index < 2 {
                assert!(bytes.starts_with(b"POST "));
                results.push(
                    serde_json::from_slice::<RuntimeValidationResult>(
                        &bytes[start..start + length],
                    )
                    .unwrap(),
                );
                if index == 0 {
                    continue;
                }
                serde_json::to_vec(&TaskAck {
                    ids: vec![request.id],
                })
                .unwrap()
            } else {
                assert!(bytes.starts_with(b"GET "));
                b"[]".to_vec()
            };
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        }
        results
    });
    assert!(poll(&[], &state, &client).await.is_err());
    let reopened = Arc::new(Mutex::new(State::open(&path).unwrap()));
    poll(&[], &reopened, &client).await.unwrap();
    let results = fixture.await.unwrap();
    assert_eq!(results, [completed.clone(), completed]);
    let floor: std::collections::BTreeMap<String, u64> = reopened
        .lock()
        .unwrap()
        .get_json("runtime_generation_floor:demo")
        .unwrap()
        .unwrap();
    assert_eq!(floor["scope:1"], 2);
    drop(reopened);
    drop(state);
    std::fs::remove_file(path).unwrap();
}

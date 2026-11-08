use super::super::{Failure, transport};
use axum::{
    Router,
    body::{Body, Bytes},
    http::Response,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{net::TcpListener, task::JoinHandle};

struct Endpoint {
    url: String,
    hits: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Endpoint {
    async fn start(response: Response<Body>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let response = Arc::new(Mutex::new(Some(response)));
        let hits = Arc::new(AtomicUsize::new(0));
        let recorded_hits = hits.clone();
        let router = Router::new().fallback(move || {
            let response = response.clone();
            let hits = recorded_hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                response.lock().unwrap().take().unwrap()
            }
        });
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self { url, hits, task }
    }
}

fn failure_only(error: Failure, code: &str) {
    assert_eq!(error.code, code);
    let retained = format!("{error:?}");
    assert!(!retained.contains("TEST_ONLY_SECRET"));
    assert!(!retained.contains("example.invalid"));
}

#[tokio::test]
async fn cloud_transport_caps_declared_and_chunked_response_bytes() {
    let declared = Response::builder()
        .header("content-length", 262145)
        .body(Body::from(vec![b'x'; 262145]))
        .unwrap();
    let streamed = Body::from_stream(futures_util::stream::iter([
        Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 131072])),
        Ok(Bytes::from(vec![b'y'; 131073])),
    ]));
    for response in [declared, Response::new(streamed)] {
        let endpoint = Endpoint::start(response).await;
        let result = transport::json(transport::client().unwrap().get(&endpoint.url)).await;
        failure_only(result.unwrap_err(), "response_too_large");
        assert_eq!(endpoint.hits.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn cloud_transport_refuses_redirect_without_forwarding_authorization() {
    let target = Endpoint::start(Response::new(Body::from("{}"))).await;
    let redirect = Endpoint::start(
        Response::builder()
            .status(307)
            .header("location", &target.url)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let result = transport::json(
        transport::client()
            .unwrap()
            .post(&redirect.url)
            .header("authorization", "TEST_ONLY_SECRET")
            .body("TEST_ONLY_SECRET"),
    )
    .await;
    failure_only(result.unwrap_err(), "redirect_refused");
    assert_eq!(redirect.hits.load(Ordering::SeqCst), 1);
    assert_eq!(target.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cloud_rate_limits_and_provider_errors_keep_only_local_categories() {
    for (retry, expected) in [("0", 60), ("999999", 3600), ("invalid", 300)] {
        let endpoint = Endpoint::start(
            Response::builder()
                .status(429)
                .header("retry-after", retry)
                .body(Body::from("TEST_ONLY_SECRET https://example.invalid/"))
                .unwrap(),
        )
        .await;
        let error = transport::json(transport::client().unwrap().get(&endpoint.url))
            .await
            .unwrap_err();
        failure_only(error, "rate_limited");
        assert_eq!(error.retry_after, expected);
    }
    for (status, provider_code, expected) in [
        (400, "OperationDenied.NoStock", "capacity_unavailable"),
        (403, "InsufficientBalance", "insufficient_balance"),
        (403, "InstanceLockedForSecurity", "resource_locked"),
        (404, "InvalidInstanceId.NotFound", "resource_not_found"),
        (400, "IncorrectInstanceStatus", "request_rejected"),
        (403, "TEST_ONLY_SECRET", "provider_rejected"),
    ] {
        let body = serde_json::json!({"Code":provider_code,"Message":"TEST_ONLY_SECRET https://example.invalid/"});
        let endpoint = Endpoint::start(
            Response::builder()
                .status(status)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
        let error = transport::power_json(transport::client().unwrap().get(&endpoint.url))
            .await
            .unwrap_err();
        failure_only(error, expected);
        assert_eq!(endpoint.hits.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn cloud_response_body_cannot_outlive_the_request_deadline() {
    let body = Body::from_stream(futures_util::stream::pending::<Result<Bytes, std::io::Error>>());
    let endpoint = Endpoint::start(Response::new(body)).await;
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        transport::json(transport::client().unwrap().get(&endpoint.url)),
    )
    .await
    .expect("production six-second deadline did not bound the body");
    failure_only(result.unwrap_err(), "network_error");
    assert!(started.elapsed() < Duration::from_secs(8));
    assert_eq!(endpoint.hits.load(Ordering::SeqCst), 1);
}

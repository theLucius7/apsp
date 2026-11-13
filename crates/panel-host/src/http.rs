//! Outbound HTTP: the one way the panel builds a client for a request it makes itself, and the
//! one bounded reading of what comes back (ADR 0133).
//!
//! Every client has the same fixed policy: no proxy (proxy environment variables are ignored),
//! no redirects (a caller that must follow one does it itself and checks every hop), no
//! automatic retries, no `Referer`, and bodies exactly as sent (nothing is decompressed inside
//! the client; a caller that accepts an encoding decodes it under its own limits). Callers
//! state only what differs per site, in [`Outbound`]. A test below keeps client construction
//! in this module.

use reqwest::{
    Client, Response, StatusCode,
    dns::Resolve,
    header::{HeaderMap, RETRY_AFTER},
    redirect::Policy,
};
use serde::de::DeserializeOwned;
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime},
};

/// What an outbound client varies per site; everything else is the fixed policy above.
#[derive(Clone, Default)]
pub struct Outbound {
    /// Limit for opening the connection, TLS included.
    pub connect_timeout: Option<Duration>,
    /// Limit for a whole exchange, from sending the request to the last byte of the body.
    pub timeout: Option<Duration>,
    /// Refuse plain-HTTP URLs.
    pub https_only: bool,
    /// Connect to these addresses for this host instead of resolving it: addresses the caller
    /// checked (see [`crate::net::check_resolved`]).
    pub pin: Option<(String, Vec<SocketAddr>)>,
    /// Resolve names with this resolver instead of the system's.
    pub resolver: Option<Arc<dyn Resolve>>,
    /// The product token the client identifies itself with; none is sent by default.
    pub product: Option<&'static str>,
}

impl Outbound {
    /// Builds the client. It fails only when the TLS backend cannot start.
    pub fn build(self) -> reqwest::Result<Client> {
        let mut builder = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .referer(false)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .https_only(self.https_only);
        if let Some(timeout) = self.connect_timeout {
            builder = builder.connect_timeout(timeout);
        }
        if let Some(timeout) = self.timeout {
            builder = builder.timeout(timeout);
        }
        if let Some((host, addresses)) = &self.pin {
            builder = builder.resolve_to_addrs(host, addresses);
        }
        if let Some(resolver) = self.resolver {
            builder = builder.dns_resolver2(resolver);
        }
        if let Some(product) = self.product {
            builder = builder.user_agent(product);
        }
        builder.build()
    }
}

/// Why a bounded body could not be read.
#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    /// The declared length or the bytes received exceed the limit.
    #[error("response body exceeds the limit")]
    TooLarge,
    /// The connection failed or timed out while the body arrived.
    #[error("response body could not be read")]
    Read(#[source] reqwest::Error),
    /// The body is not JSON of the expected shape.
    #[error("response body is not the expected JSON")]
    Invalid,
}

/// Reads at most `limit` bytes of a response body, whatever its declared length says.
pub async fn body(mut response: Response, limit: usize) -> Result<Vec<u8>, BodyError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(BodyError::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(BodyError::Read)? {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(BodyError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Reads a JSON body of at most `limit` bytes.
pub async fn json<T: DeserializeOwned>(response: Response, limit: usize) -> Result<T, BodyError> {
    serde_json::from_slice(&body(response, limit).await?).map_err(|_| BodyError::Invalid)
}

/// What a refused answer says by its status alone, in the categories provider clients report.
/// Each client names the categories in its own vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// 401 or 403: the credentials were not accepted.
    Credentials,
    /// 404.
    Missing,
    /// 400 or 409: the request itself was refused.
    Rejected,
    /// 429: see [`RetryAfter`] for how long to wait.
    RateLimited,
    /// 3xx: clients built here never follow a redirect.
    Redirect,
    /// 5xx.
    Unavailable,
    /// Any other status outside 2xx.
    Other,
}

impl Refusal {
    /// The category of a status, or `None` for a success (2xx).
    pub fn of(status: StatusCode) -> Option<Self> {
        Some(match status.as_u16() {
            200..=299 => return None,
            401 | 403 => Self::Credentials,
            404 => Self::Missing,
            400 | 409 => Self::Rejected,
            429 => Self::RateLimited,
            300..=399 => Self::Redirect,
            500..=599 => Self::Unavailable,
            _ => Self::Other,
        })
    }
}

/// How long a peer asked the panel to wait before trying again, in seconds, kept within the
/// bounds the caller allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryAfter {
    min: u64,
    max: u64,
}

impl RetryAfter {
    pub const fn new(min: u64, max: u64) -> Self {
        Self { min, max }
    }

    /// A `Retry-After` value: delay-seconds or an HTTP date, counted from `now`. `None` when
    /// it is neither; a signed or fractional number is not a delay. Nothing of the value is
    /// kept beyond the number.
    pub fn parse(self, value: &str, now: SystemTime) -> Option<i64> {
        let value = value.trim();
        let seconds = if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
            value.parse::<u64>().ok()?
        } else {
            let duration = httpdate::parse_http_date(value)
                .ok()?
                .duration_since(now)
                .unwrap_or_default();
            duration
                .as_secs()
                .saturating_add(u64::from(duration.subsec_nanos() != 0))
        };
        Some(seconds.clamp(self.min, self.max) as i64)
    }

    /// The `Retry-After` header of an answer, when it has a valid one.
    pub fn header(self, headers: &HeaderMap, now: SystemTime) -> Option<i64> {
        let value = headers.get(RETRY_AFTER)?.to_str().ok()?;
        self.parse(value, now)
    }

    /// A delay the peer gave as a number of seconds outside the header (in its JSON answer).
    pub fn seconds(self, seconds: i64) -> i64 {
        seconds.clamp(self.min as i64, self.max as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::{Body, Bytes},
        http::Response as Answer,
    };
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
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
        async fn start(answer: Answer<Body>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let answer = Arc::new(Mutex::new(Some(answer)));
            let hits = Arc::new(AtomicUsize::new(0));
            let counted = hits.clone();
            let router = Router::new().fallback(move || {
                let answer = answer.clone();
                let hits = counted.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    answer.lock().unwrap().take().unwrap()
                }
            });
            let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            Self { url, hits, task }
        }
    }

    fn client() -> Client {
        Outbound {
            timeout: Some(Duration::from_secs(5)),
            ..Outbound::default()
        }
        .build()
        .unwrap()
    }

    #[tokio::test]
    async fn clients_return_redirects_instead_of_following_them() {
        let target = Endpoint::start(Answer::new(Body::from("{}"))).await;
        let redirect = Endpoint::start(
            Answer::builder()
                .status(307)
                .header("location", &target.url)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        let response = client()
            .post(&redirect.url)
            .header("authorization", "TEST_ONLY_SECRET")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(Refusal::of(response.status()), Some(Refusal::Redirect));
        assert_eq!(redirect.hits.load(Ordering::SeqCst), 1);
        assert_eq!(target.hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn bodies_are_capped_by_declared_and_received_length() {
        const LIMIT: usize = 1024;
        let declared = Answer::builder()
            .header("content-length", LIMIT + 1)
            .body(Body::from(vec![b'x'; LIMIT + 1]))
            .unwrap();
        let streamed = Answer::new(Body::from_stream(futures_util::stream::iter([
            Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; LIMIT / 2])),
            Ok(Bytes::from(vec![b'y'; LIMIT / 2 + 1])),
        ])));
        for answer in [declared, streamed] {
            let endpoint = Endpoint::start(answer).await;
            let response = client().get(&endpoint.url).send().await.unwrap();
            assert!(matches!(
                body(response, LIMIT).await,
                Err(BodyError::TooLarge)
            ));
        }
        let exact = Endpoint::start(Answer::new(Body::from(vec![b' '; LIMIT]))).await;
        let response = client().get(&exact.url).send().await.unwrap();
        assert_eq!(body(response, LIMIT).await.unwrap().len(), LIMIT);
        let invalid = Endpoint::start(Answer::new(Body::from("not json"))).await;
        let response = client().get(&invalid.url).send().await.unwrap();
        assert!(matches!(
            json::<serde_json::Value>(response, LIMIT).await,
            Err(BodyError::Invalid)
        ));
        let valid = Endpoint::start(Answer::new(Body::from(r#"{"valid":true}"#))).await;
        let response = client().get(&valid.url).send().await.unwrap();
        assert_eq!(
            json::<serde_json::Value>(response, LIMIT).await.unwrap(),
            serde_json::json!({"valid":true})
        );
    }

    #[test]
    fn statuses_fall_into_the_shared_categories() {
        for (status, expected) in [
            (200, None),
            (204, None),
            (301, Some(Refusal::Redirect)),
            (308, Some(Refusal::Redirect)),
            (400, Some(Refusal::Rejected)),
            (401, Some(Refusal::Credentials)),
            (403, Some(Refusal::Credentials)),
            (404, Some(Refusal::Missing)),
            (405, Some(Refusal::Other)),
            (409, Some(Refusal::Rejected)),
            (429, Some(Refusal::RateLimited)),
            (500, Some(Refusal::Unavailable)),
            (503, Some(Refusal::Unavailable)),
            (101, Some(Refusal::Other)),
        ] {
            assert_eq!(
                Refusal::of(StatusCode::from_u16(status).unwrap()),
                expected,
                "{status}"
            );
        }
    }

    #[test]
    fn retry_after_accepts_http_dates_and_unsigned_seconds_within_bounds() {
        let now = httpdate::parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT").unwrap();
        let channel = RetryAfter::new(1, 86_400);
        for (value, expected) in [
            ("123", Some(123)),
            ("0", Some(1)),
            ("999999", Some(86_400)),
            ("Sun, 06 Nov 1994 08:51:40 GMT", Some(123)),
            ("Sunday, 06-Nov-94 08:51:40 GMT", Some(123)),
            ("Sun Nov  6 08:51:40 1994", Some(123)),
            ("Sun, 06 Nov 1994 08:49:36 GMT", Some(1)),
            ("-1", None),
            ("+1", None),
            ("1.5", None),
            ("", None),
            ("TEST_ONLY_SECRET", None),
        ] {
            assert_eq!(channel.parse(value, now), expected, "{value}");
        }
        assert_eq!(
            channel.parse(
                "Sun, 06 Nov 1994 08:51:40 GMT",
                now + Duration::from_millis(1)
            ),
            Some(123)
        );
        let provider = RetryAfter::new(60, 3600);
        for (value, expected) in [
            ("900", Some(900)),
            ("0", Some(60)),
            ("99999", Some(3600)),
            ("Sun, 06 Nov 1994 09:04:37 GMT", Some(900)),
            ("invalid", None),
        ] {
            assert_eq!(provider.parse(value, now), expected, "{value}");
        }
        let mut headers = HeaderMap::new();
        assert_eq!(provider.header(&headers, now), None);
        headers.insert(RETRY_AFTER, "120".parse().unwrap());
        assert_eq!(provider.header(&headers, now), Some(120));
        assert_eq!(channel.seconds(123), 123);
        assert_eq!(channel.seconds(-5), 1);
        assert_eq!(channel.seconds(10_000_000), 86_400);
    }

    /// The Rust sources below `crates/`, without build output.
    fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).expect("readable source directory") {
            let entry = entry.expect("directory entry");
            let kind = entry.file_type().expect("file type");
            let path = entry.path();
            if kind.is_dir() && entry.file_name() != "target" {
                sources(&path, found);
            } else if kind.is_file() && path.extension().is_some_and(|extension| extension == "rs")
            {
                found.push(path);
            }
        }
    }

    /// The Agent, its adapters and its probe are another program with their own network
    /// policy; every other crate is the panel.
    fn agent_side(relative: &Path) -> bool {
        let krate = relative
            .components()
            .next()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        matches!(krate.as_str(), "agent" | "agent-core" | "tcp-probe")
            || krate.starts_with("adapter-")
    }

    /// A client built anywhere but here would follow redirects, take proxies from the
    /// environment and retry on its own; tests and fixtures included, every panel client goes
    /// through [`Outbound`].
    #[test]
    fn outbound_clients_are_built_only_here() {
        const CONSTRUCTORS: [&str; 5] = [
            "Client::builder",
            "ClientBuilder",
            "Client::new()",
            "Client::default()",
            "reqwest::get(",
        ];
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates directory");
        let mut files = Vec::new();
        sources(crates, &mut files);
        assert!(files.len() > 100, "the workspace sources were not found");
        let mut misplaced = Vec::new();
        for path in &files {
            let relative = path.strip_prefix(crates).expect("path under crates/");
            if agent_side(relative) || relative == Path::new("panel-host/src/http.rs") {
                continue;
            }
            let source = fs::read_to_string(path).expect("readable source");
            for (index, line) in source.lines().enumerate() {
                if CONSTRUCTORS.iter().any(|pattern| line.contains(pattern)) {
                    misplaced.push(format!("{}:{}: {}", path.display(), index + 1, line.trim()));
                }
            }
        }
        assert!(
            misplaced.is_empty(),
            "build outbound HTTP clients with apsp_panel_host::http::Outbound:\n{}",
            misplaced.join("\n")
        );
        assert!(agent_side(Path::new("adapter-sdk/src/lib.rs")));
        assert!(!agent_side(Path::new("panel/tests/foundation.rs")));
    }
}

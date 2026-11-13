//! Globalping (https://globalping.io): TCP connection tests to an address from probes in
//! chosen places, behind an injectable transport so that tests never touch the network.
//!
//! Measurement targets are typed as IP literals, so a configured hostname can never reach
//! the third party. `ipVersion` is deliberately not sent: the service rejects it for IPv6
//! literals and infers the family from the target. Callers choose the places, the number of
//! probes and handshakes, the token and the HTTP client. A measurement is either polled here
//! until every probe has answered ([`Client::tcp_ping`]), or started and read once at a time
//! by a caller that polls on its own schedule ([`Client::start`], [`Client::measurement`]).

use super::{Failure, transport};
use apsp_panel_host::http::{self, BodyError, Refusal};
use reqwest::StatusCode;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};
use std::{future::Future, net::IpAddr, time::Duration};

const BASE_URL: &str = "https://api.globalping.io";
const POLL_INTERVAL: Duration = Duration::from_millis(1500);
/// About a minute of polling; a measurement normally finishes in a few seconds.
const MAX_POLLS: u32 = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    pub path: String,
    pub body: Option<Value>,
    pub bearer: Option<String>,
}

/// An answer: its status and, for a success (2xx), its body.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Why a request brought back no answer to judge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportFailure {
    /// The request was not sent, or the answer was cut off.
    Network,
    /// The answer was larger than the transport reads.
    TooLarge,
}

/// Sends one API request; tests replace this so no network is touched.
pub trait Transport: Send + Sync {
    fn send(
        &self,
        request: Request,
    ) -> impl Future<Output = Result<Response, TransportFailure>> + Send;
}

/// The service itself, through a client the caller built (its timeouts, HTTPS rule and
/// product name), reading at most `limit` bytes of an answer. Like every provider call, an
/// answer is judged by its status first: the body of a refused answer is never read.
#[derive(Clone)]
pub struct Live {
    client: reqwest::Client,
    base: String,
    limit: usize,
}

impl Live {
    pub fn new(client: reqwest::Client, limit: usize) -> Self {
        Self {
            client,
            base: BASE_URL.into(),
            limit,
        }
    }

    /// A stand-in for the service on this machine, for tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn local(client: reqwest::Client, endpoint: &str, limit: usize) -> Self {
        assert_eq!(
            reqwest::Url::parse(endpoint).unwrap().host_str(),
            Some("127.0.0.1")
        );
        Self {
            client,
            base: endpoint.trim_end_matches('/').into(),
            limit,
        }
    }
}

impl Transport for Live {
    async fn send(&self, request: Request) -> Result<Response, TransportFailure> {
        let url = format!("{}{}", self.base, request.path);
        let mut builder = match request.method {
            Method::Get => self.client.get(url),
            Method::Post => self.client.post(url),
        };
        if let Some(body) = &request.body {
            builder = builder.json(body);
        }
        if let Some(token) = &request.bearer {
            builder = builder.bearer_auth(token);
        }
        let response = builder
            .send()
            .await
            .map_err(|_| TransportFailure::Network)?;
        let status = response.status();
        if !status.is_success() {
            return Ok(Response {
                status: status.as_u16(),
                body: Vec::new(),
            });
        }
        let body = http::body(response, self.limit)
            .await
            .map_err(|error| match error {
                BodyError::TooLarge => TransportFailure::TooLarge,
                BodyError::Read(_) | BodyError::Invalid => TransportFailure::Network,
            })?;
        Ok(Response {
            status: status.as_u16(),
            body,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasureError {
    /// No answer to judge.
    Network(TransportFailure),
    /// 429: the allowance of the current window is spent.
    RateLimited,
    /// Any other status outside 2xx.
    Rejected(u16),
    /// The answer is not what the API documents.
    Malformed,
    /// The measurement did not finish while it was polled.
    Timeout,
}

impl MeasureError {
    /// A stable code for storage. Never contains the address or the token.
    pub fn code(self) -> String {
        match self {
            Self::Network(_) => "network".into(),
            Self::RateLimited => "rate_limited".into(),
            Self::Rejected(status) => format!("http_{status}"),
            Self::Malformed => "malformed_response".into(),
            Self::Timeout => "timeout".into(),
        }
    }
}

/// The words every cloud provider call reports failures in (see [`transport`]): statuses fall
/// into the shared categories, a status outside them is `provider_rejected`, and a 429 waits
/// the default time (the service's own wait is not read).
impl From<MeasureError> for Failure {
    fn from(error: MeasureError) -> Self {
        const OTHER: &str = "provider_rejected";
        match error {
            MeasureError::Network(TransportFailure::Network) => "network_error".into(),
            MeasureError::Network(TransportFailure::TooLarge) => "response_too_large".into(),
            MeasureError::RateLimited => transport::refused(Refusal::RateLimited, OTHER),
            MeasureError::Rejected(status) => StatusCode::from_u16(status)
                .ok()
                .and_then(Refusal::of)
                .map_or_else(
                    || OTHER.into(),
                    |category| transport::refused(category, OTHER),
                ),
            MeasureError::Malformed => "invalid_response".into(),
            // Only a measurement polled here runs out of time: the service did not finish it.
            MeasureError::Timeout => "provider_unavailable".into(),
        }
    }
}

/// The measurement allowance the service reports: `limit` tests a window, `remaining` in the
/// current one, which ends in `reset_secs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub limit: u32,
    pub remaining: u32,
    pub reset_secs: u32,
}

/// One reading of a measurement.
#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    /// "in-progress" until every probe has answered, then "finished".
    pub status: String,
    /// Every probe's answer so far; `None` when the reading has no results.
    pub results: Option<Vec<Answer>>,
}

/// One probe's answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub network: String,
    pub city: String,
    pub country: Option<String>,
    /// The probe's counters; `None` when it reported none (it failed, or is still running).
    pub stats: Option<Stats>,
}

/// A probe's counters: handshakes completed, their average time in milliseconds and the
/// share lost in percent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
pub struct Stats {
    #[serde(default)]
    pub rcv: Option<f64>,
    #[serde(default)]
    pub avg: Option<f64>,
    /// Read from any JSON value: a number is the share lost, anything else is unknown.
    #[serde(default, deserialize_with = "number")]
    pub loss: Option<f64>,
}

/// What one probe measured, as [`Client::tcp_ping`] reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeResult {
    pub network: String,
    pub city: String,
    pub country: Option<String>,
    pub received: f64,
    pub avg_ms: Option<f64>,
}

#[derive(Deserialize)]
struct LimitsBody {
    #[serde(rename = "rateLimit")]
    rate_limit: RateLimit,
}
#[derive(Deserialize)]
struct RateLimit {
    measurements: RateLimitMeasurements,
}
#[derive(Deserialize)]
struct RateLimitMeasurements {
    create: RateLimitCreate,
}
#[derive(Deserialize)]
struct RateLimitCreate {
    limit: u32,
    remaining: u32,
    reset: u32,
}

#[derive(Deserialize)]
struct Created {
    id: String,
}

#[derive(Deserialize)]
struct Reading {
    status: String,
    #[serde(default, deserialize_with = "present")]
    results: Option<Vec<ResultEntry>>,
}
#[derive(Deserialize)]
struct ResultEntry {
    #[serde(default)]
    probe: ProbeMeta,
    #[serde(default)]
    result: ResultBody,
}
#[derive(Default, Deserialize)]
struct ProbeMeta {
    #[serde(default)]
    network: String,
    #[serde(default)]
    city: String,
    #[serde(default)]
    country: Option<String>,
}
#[derive(Default, Deserialize)]
struct ResultBody {
    #[serde(default)]
    stats: Option<Stats>,
}

impl From<Reading> for Measurement {
    fn from(reading: Reading) -> Self {
        Self {
            status: reading.status,
            results: reading.results.map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| Answer {
                        network: entry.probe.network,
                        city: entry.probe.city,
                        country: entry.probe.country,
                        stats: entry.result.stats,
                    })
                    .collect()
            }),
        }
    }
}

/// A field that may be absent but, when present, must hold its type: `null` is refused.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn number<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    Ok(Value::deserialize(deserializer)?.as_f64())
}

fn valid_measurement_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}

fn clipped(value: String) -> String {
    value.chars().take(128).collect()
}

pub struct Client<'a, T> {
    transport: &'a T,
    token: Option<&'a str>,
    poll_interval: Duration,
}

impl<'a, T: Transport> Client<'a, T> {
    /// Requests carry `token` as a bearer credential when one is given, and none otherwise.
    pub fn new(transport: &'a T, token: Option<&'a str>) -> Self {
        Self {
            transport,
            token,
            poll_interval: POLL_INTERVAL,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn fast_polling(mut self) -> Self {
        self.poll_interval = Duration::from_millis(1);
        self
    }

    async fn call(
        &self,
        method: Method,
        path: String,
        body: Option<Value>,
    ) -> Result<Vec<u8>, MeasureError> {
        let response = self
            .transport
            .send(Request {
                method,
                path,
                body,
                bearer: self.token.map(str::to_owned),
            })
            .await
            .map_err(MeasureError::Network)?;
        match response.status {
            200..=299 => Ok(response.body),
            429 => Err(MeasureError::RateLimited),
            status => Err(MeasureError::Rejected(status)),
        }
    }

    pub async fn limits(&self) -> Result<Limits, MeasureError> {
        let body = self.call(Method::Get, "/v1/limits".into(), None).await?;
        let parsed: LimitsBody =
            serde_json::from_slice(&body).map_err(|_| MeasureError::Malformed)?;
        let create = parsed.rate_limit.measurements.create;
        Ok(Limits {
            limit: create.limit,
            remaining: create.remaining,
            reset_secs: create.reset,
        })
    }

    /// Starts a TCP connection test of `target:port`: `packets` handshakes from every probe
    /// the `locations` select (the API's location objects), and at most `limit` probes in all
    /// when one is given. Returns the measurement's identifier.
    pub async fn start(
        &self,
        target: IpAddr,
        port: u16,
        packets: u32,
        locations: Value,
        limit: Option<u32>,
    ) -> Result<String, MeasureError> {
        let mut body = Map::new();
        body.insert("type".into(), "ping".into());
        body.insert("target".into(), target.to_string().into());
        if let Some(limit) = limit {
            body.insert("limit".into(), limit.into());
        }
        body.insert("locations".into(), locations);
        body.insert(
            "measurementOptions".into(),
            json!({"protocol":"TCP","port":port,"packets":packets}),
        );
        let created = self
            .call(Method::Post, "/v1/measurements".into(), Some(body.into()))
            .await?;
        let created: Created =
            serde_json::from_slice(&created).map_err(|_| MeasureError::Malformed)?;
        if !valid_measurement_id(&created.id) {
            return Err(MeasureError::Malformed);
        }
        Ok(created.id)
    }

    /// One reading of a measurement. An identifier that could alter the request path is
    /// refused without a request.
    pub async fn measurement(&self, id: &str) -> Result<Measurement, MeasureError> {
        if !valid_measurement_id(id) {
            return Err(MeasureError::Malformed);
        }
        let body = self
            .call(Method::Get, format!("/v1/measurements/{id}"), None)
            .await?;
        let reading: Reading =
            serde_json::from_slice(&body).map_err(|_| MeasureError::Malformed)?;
        Ok(reading.into())
    }

    /// Runs a TCP connection test (see [`Client::start`], with no overall probe limit) and
    /// polls it until every probe has answered. A probe without counters is left out: it
    /// reported nothing about the target.
    pub async fn tcp_ping(
        &self,
        ip: IpAddr,
        port: u16,
        packets: u32,
        locations: Value,
    ) -> Result<Vec<ProbeResult>, MeasureError> {
        let id = self.start(ip, port, packets, locations, None).await?;
        for _ in 0..MAX_POLLS {
            tokio::time::sleep(self.poll_interval).await;
            let measurement = self.measurement(&id).await?;
            if measurement.status == "finished" {
                return Ok(measurement
                    .results
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|answer| {
                        let stats = answer.stats?;
                        Some(ProbeResult {
                            network: clipped(answer.network),
                            city: clipped(answer.city),
                            country: answer.country,
                            received: stats.rcv?,
                            avg_ms: stats.avg,
                        })
                    })
                    .collect());
            }
        }
        Err(MeasureError::Timeout)
    }
}

/// A scripted stand-in for the service.
#[cfg(any(test, feature = "test-support"))]
pub mod fake {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex};

    /// Replays scripted responses and records every request it receives.
    #[derive(Default)]
    pub struct Scripted {
        pub requests: Mutex<Vec<Request>>,
        pub replies: Mutex<VecDeque<Result<(u16, Value), TransportFailure>>>,
    }

    impl Scripted {
        pub fn reply(self, status: u16, body: Value) -> Self {
            self.replies.lock().unwrap().push_back(Ok((status, body)));
            self
        }
        pub fn fail(self, failure: TransportFailure) -> Self {
            self.replies.lock().unwrap().push_back(Err(failure));
            self
        }
    }

    impl Transport for Scripted {
        async fn send(&self, request: Request) -> Result<Response, TransportFailure> {
            self.requests.lock().unwrap().push(request);
            let reply = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("a scripted reply");
            reply.map(|(status, body)| Response {
                status,
                body: serde_json::to_vec(&body).unwrap(),
            })
        }
    }

    pub fn probe(network: &str, country: &str, rcv: u32, avg: Option<f64>) -> Value {
        json!({"probe":{"network":network,"city":"Fixture City","country":country},
            "result":{"status":"finished","stats":{"rcv":rcv,"avg":avg}}})
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{Scripted, probe};
    use super::*;
    use apsp_panel_host::http::Outbound;

    const TARGET: &str = "198.51.100.7";

    fn ip() -> IpAddr {
        TARGET.parse().unwrap()
    }

    fn china() -> Value {
        json!([
            {"magic":"China Telecom","limit":2},
            {"magic":"China Unicom","limit":1},
            {"magic":"China Mobile","limit":1},
        ])
    }

    fn europe(limit: u32) -> Value {
        json!([{"magic":"Europe","limit":limit}])
    }

    #[tokio::test]
    async fn a_measurement_is_submitted_with_the_verified_shape_and_polled_to_the_end() {
        let transport = Scripted::default()
            .reply(202, json!({"id":"abc123","probesCount":4}))
            .reply(200, json!({"status":"in-progress","results":[]}))
            .reply(
                200,
                json!({"status":"finished","results":[
                    probe("China Telecom Group","CN",3,Some(31.5)),
                    probe("China Mobile","CN",0,None),
                ]}),
            );
        let results = Client::new(&transport, None)
            .fast_polling()
            .tcp_ping(ip(), 8443, 3, china())
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].received, 3.0);
        assert_eq!(results[0].avg_ms, Some(31.5));
        assert_eq!(results[1].received, 0.0);
        assert_eq!(results[1].avg_ms, None);
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].method, Method::Post);
        assert_eq!(requests[0].path, "/v1/measurements");
        assert_eq!(requests[1].path, "/v1/measurements/abc123");
        assert_eq!(
            requests[0].body,
            Some(json!({
                "type":"ping","target":TARGET,
                "locations":[
                    {"magic":"China Telecom","limit":2},
                    {"magic":"China Unicom","limit":1},
                    {"magic":"China Mobile","limit":1},
                ],
                "measurementOptions":{"protocol":"TCP","port":8443,"packets":3},
            }))
        );
    }

    #[tokio::test]
    async fn a_started_measurement_caps_its_probes_and_is_read_once_with_every_answer() {
        let transport = Scripted::default()
            .reply(202, json!({"id":"abc123"}))
            .reply(
                200,
                json!({"status":"finished","results":[
                    {"probe":{"network":"Example Telecom"},"result":{"stats":{"loss":0}}},
                    {"probe":{"network":"Example Unicom"},"result":{"status":"failed"}},
                    {"probe":{},"result":{"stats":{"loss":66.6,"rcv":1}}},
                    {"probe":{"network":"Example Mobile"},"result":{"stats":{"loss":"unknown"}}},
                ]}),
            )
            .reply(200, json!({"status":"in-progress"}));
        let client = Client::new(&transport, None);
        let id = client
            .start(ip(), 443, 4, json!([{"country":"CN"}]), Some(14))
            .await
            .unwrap();
        assert_eq!(id, "abc123");
        let finished = client.measurement(&id).await.unwrap();
        assert_eq!(finished.status, "finished");
        let results = finished.results.unwrap();
        // Probes without counters are kept: the caller decides what they mean.
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].network, "Example Telecom");
        assert_eq!(results[0].stats.unwrap().loss, Some(0.0));
        assert!(results[1].stats.is_none());
        assert_eq!(results[2].network, "");
        assert_eq!(results[2].stats.unwrap().loss, Some(66.6));
        assert_eq!(results[2].stats.unwrap().rcv, Some(1.0));
        assert_eq!(results[3].stats.unwrap().loss, None);
        let running = client.measurement(&id).await.unwrap();
        assert_eq!(running.status, "in-progress");
        assert!(running.results.is_none());
        let requests = transport.requests.lock().unwrap();
        assert_eq!(
            requests[0].body,
            Some(json!({
                "type":"ping","target":TARGET,"limit":14,
                "locations":[{"country":"CN"}],
                "measurementOptions":{"protocol":"TCP","port":443,"packets":4},
            }))
        );
        assert_eq!(requests[1].path, "/v1/measurements/abc123");
        assert_eq!(requests.len(), 3);
    }

    #[tokio::test]
    async fn readings_without_a_status_or_with_null_results_are_malformed() {
        let transport = Scripted::default()
            .reply(200, json!({"results":[]}))
            .reply(200, json!({"status":"finished","results":null}));
        let client = Client::new(&transport, None);
        for _ in 0..2 {
            assert_eq!(
                client.measurement("abc123").await.unwrap_err(),
                MeasureError::Malformed
            );
        }
    }

    #[tokio::test]
    async fn ipv6_literals_are_sent_bare_and_without_an_ip_version() {
        let transport = Scripted::default()
            .reply(202, json!({"id":"six"}))
            .reply(200, json!({"status":"finished","results":[]}));
        Client::new(&transport, None)
            .fast_polling()
            .tcp_ping("2001:db8::1".parse().unwrap(), 443, 3, europe(2))
            .await
            .unwrap();
        let requests = transport.requests.lock().unwrap();
        let body = requests[0].body.as_ref().unwrap();
        assert_eq!(body["target"], "2001:db8::1");
        assert!(body.get("ipVersion").is_none());
        assert!(body["measurementOptions"].get("ipVersion").is_none());
        assert!(body.get("limit").is_none());
        assert_eq!(body["locations"], json!([{"magic":"Europe","limit":2}]));
    }

    #[tokio::test]
    async fn the_token_travels_only_as_a_bearer_credential_when_configured() {
        let transport = Scripted::default().reply(
            200,
            json!({"rateLimit":{"measurements":{"create":{"type":"ip","limit":250,"remaining":240,"reset":1200}}}}),
        );
        let limits = Client::new(&transport, Some("fixture-token"))
            .limits()
            .await
            .unwrap();
        assert_eq!(
            limits,
            Limits {
                limit: 250,
                remaining: 240,
                reset_secs: 1200
            }
        );
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests[0].bearer.as_deref(), Some("fixture-token"));
        assert_eq!(requests[0].path, "/v1/limits");
        assert!(requests[0].body.is_none());
    }

    #[tokio::test]
    async fn anonymous_requests_send_no_credential() {
        let transport = Scripted::default().reply(
            200,
            json!({"rateLimit":{"measurements":{"create":{"limit":250,"remaining":250,"reset":0}}}}),
        );
        Client::new(&transport, None).limits().await.unwrap();
        assert!(transport.requests.lock().unwrap()[0].bearer.is_none());
    }

    #[tokio::test]
    async fn failures_map_to_stable_codes_and_provider_failures_without_the_address() {
        async fn failure(transport: Scripted) -> MeasureError {
            Client::new(&transport, None)
                .fast_polling()
                .tcp_ping(ip(), 443, 3, europe(1))
                .await
                .unwrap_err()
        }
        let cases = [
            (
                failure(Scripted::default().reply(429, json!({}))).await,
                "rate_limited",
                "rate_limited",
            ),
            (
                failure(Scripted::default().reply(400, json!({"error":TARGET}))).await,
                "http_400",
                "provider_rejected",
            ),
            (
                failure(Scripted::default().reply(401, json!({}))).await,
                "http_401",
                "authentication_failed",
            ),
            (
                failure(Scripted::default().reply(404, json!({}))).await,
                "http_404",
                "resource_missing",
            ),
            (
                failure(Scripted::default().reply(503, json!({}))).await,
                "http_503",
                "provider_unavailable",
            ),
            (
                failure(Scripted::default().reply(301, json!({}))).await,
                "http_301",
                "redirect_refused",
            ),
            (
                failure(Scripted::default().reply(418, json!({}))).await,
                "http_418",
                "provider_rejected",
            ),
            (
                failure(Scripted::default().fail(TransportFailure::Network)).await,
                "network",
                "network_error",
            ),
            (
                failure(Scripted::default().fail(TransportFailure::TooLarge)).await,
                "network",
                "response_too_large",
            ),
            (
                failure(Scripted::default().reply(202, json!({"id":"../limits"}))).await,
                "malformed_response",
                "invalid_response",
            ),
            (
                failure(Scripted::default().reply(202, json!({"unexpected":true}))).await,
                "malformed_response",
                "invalid_response",
            ),
        ];
        for (error, code, provider) in cases {
            assert_eq!(error.code(), code);
            assert!(!error.code().contains(TARGET));
            let failure = Failure::from(error);
            assert_eq!(failure.code, provider);
            // Cloud flows wait the default time after a 429 and retry others at once.
            let wait = if provider == "rate_limited" { 300 } else { 0 };
            assert_eq!(failure.retry_after, wait);
        }
        assert_eq!(
            Failure::from(MeasureError::Timeout).code,
            "provider_unavailable"
        );
    }

    #[tokio::test]
    async fn identifiers_that_could_alter_the_request_path_are_never_sent() {
        let transport = Scripted::default();
        let client = Client::new(&transport, None);
        for invalid in ["", "../limits", "a/b", "a?b", &"a".repeat(129)] {
            assert_eq!(
                client.measurement(invalid).await.unwrap_err(),
                MeasureError::Malformed,
                "{invalid}"
            );
        }
        assert!(transport.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn polling_is_bounded_when_a_measurement_never_finishes() {
        let mut transport = Scripted::default().reply(202, json!({"id":"slow"}));
        for _ in 0..MAX_POLLS {
            transport = transport.reply(200, json!({"status":"in-progress"}));
        }
        let error = Client::new(&transport, None)
            .fast_polling()
            .tcp_ping(ip(), 443, 3, europe(1))
            .await
            .unwrap_err();
        assert_eq!(error, MeasureError::Timeout);
        assert_eq!(
            transport.requests.lock().unwrap().len(),
            MAX_POLLS as usize + 1
        );
    }

    #[tokio::test]
    async fn probes_without_counters_are_not_counted_as_failures() {
        let transport = Scripted::default().reply(202, json!({"id":"a"})).reply(
            200,
            json!({"status":"finished","results":[
                {"probe":{"network":"China Unicom"},"result":{"status":"failed"}},
                probe("China Telecom","CN",0,None),
            ]}),
        );
        let results = Client::new(&transport, None)
            .fast_polling()
            .tcp_ping(ip(), 443, 3, china())
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].network, "China Telecom");
    }

    #[tokio::test]
    async fn malformed_limits_are_an_error_rather_than_an_open_budget() {
        let transport = Scripted::default().reply(200, json!({"rateLimit":{}}));
        assert_eq!(
            Client::new(&transport, None).limits().await.unwrap_err(),
            MeasureError::Malformed
        );
    }

    #[tokio::test]
    async fn the_live_transport_reads_successful_answers_within_its_limit_only() {
        use axum::{
            Router,
            http::{HeaderMap, StatusCode as Status},
            routing::{get, post},
        };
        let app = Router::new()
            .route(
                "/v1/measurements",
                post(|headers: HeaderMap, body: String| async move {
                    assert_eq!(headers["authorization"], "Bearer fixture-token");
                    assert_eq!(
                        serde_json::from_str::<Value>(&body).unwrap()["target"],
                        TARGET
                    );
                    (Status::ACCEPTED, r#"{"id":"abc123"}"#)
                }),
            )
            .route(
                "/v1/measurements/abc123",
                get(|| async { "x".repeat(2048) }),
            )
            .route(
                "/v1/limits",
                get(|| async { (Status::TOO_MANY_REQUESTS, "x".repeat(2048)) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let http = Outbound {
            timeout: Some(Duration::from_secs(5)),
            ..Outbound::default()
        }
        .build()
        .unwrap();
        let transport = Live::local(http, &endpoint, 1024);
        let client = Client::new(&transport, Some("fixture-token"));
        let id = client.start(ip(), 443, 3, europe(1), None).await.unwrap();
        assert_eq!(id, "abc123");
        assert_eq!(
            client.measurement(&id).await.unwrap_err(),
            MeasureError::Network(TransportFailure::TooLarge)
        );
        // A refused answer is judged by its status; its body is not read.
        assert_eq!(
            client.limits().await.unwrap_err(),
            MeasureError::RateLimited
        );
        server.abort();
    }

    /// Checks the real client against the real service. Point
    /// `APSP_GLOBALPING_SMOKE_TARGET=<ip>:<port>` at an address you may probe.
    #[tokio::test]
    #[ignore = "calls the real Globalping API and spends measurements of its allowance"]
    async fn real_service_answers_in_the_expected_shape() {
        let target: std::net::SocketAddr = std::env::var("APSP_GLOBALPING_SMOKE_TARGET")
            .expect("set APSP_GLOBALPING_SMOKE_TARGET")
            .parse()
            .unwrap();
        let http = Outbound {
            connect_timeout: Some(Duration::from_secs(10)),
            timeout: Some(Duration::from_secs(20)),
            https_only: true,
            ..Outbound::default()
        }
        .build()
        .unwrap();
        let transport = Live::new(http, 1024 * 1024);
        let client = Client::new(&transport, None);
        assert!(client.limits().await.unwrap().limit > 0);
        let china = client
            .tcp_ping(target.ip(), target.port(), 3, china())
            .await
            .unwrap();
        assert!(!china.is_empty());
        assert!(
            china
                .iter()
                .any(|probe| probe.country.as_deref() == Some("CN"))
        );
        let overseas = client
            .tcp_ping(target.ip(), target.port(), 3, europe(2))
            .await
            .unwrap();
        assert!(overseas.iter().any(|probe| probe.received > 0.0));
    }
}

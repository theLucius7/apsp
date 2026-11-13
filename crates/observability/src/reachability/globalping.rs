//! Minimal Globalping REST client behind an injectable transport.
//!
//! Measurement targets are typed as IP literals, so a configured hostname can
//! never reach the third party. `ipVersion` is deliberately not sent: the
//! service rejects it for IPv6 literals and infers the family from the target.

use super::budget::Limits;
use apsp_panel_host::http::{self, Outbound};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{future::Future, net::IpAddr, time::Duration};

const BASE_URL: &str = "https://api.globalping.io";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const PACKETS: u32 = 3;
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

#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub struct TransportFailure;

/// Sends one API request; tests replace this so no network is touched.
pub trait Transport: Send + Sync {
    fn send(
        &self,
        request: Request,
    ) -> impl Future<Output = Result<Response, TransportFailure>> + Send;
}

pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            client: Outbound {
                connect_timeout: Some(Duration::from_secs(10)),
                timeout: Some(Duration::from_secs(20)),
                https_only: true,
                product: Some(concat!(
                    "apsp-panel-reachability/",
                    env!("CARGO_PKG_VERSION")
                )),
                ..Outbound::default()
            }
            .build()?,
        })
    }
}

impl Transport for ReqwestTransport {
    async fn send(&self, request: Request) -> Result<Response, TransportFailure> {
        let url = format!("{BASE_URL}{}", request.path);
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
        let response = builder.send().await.map_err(|_| TransportFailure)?;
        let status = response.status().as_u16();
        let body = http::body(response, MAX_RESPONSE_BYTES)
            .await
            .map_err(|_| TransportFailure)?;
        Ok(Response { status, body })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasureError {
    Network,
    RateLimited,
    Rejected(u16),
    Malformed,
    Timeout,
}

impl MeasureError {
    /// A stable code for storage. Never contains the address or the token.
    pub fn code(self) -> String {
        match self {
            Self::Network => "network".into(),
            Self::RateLimited => "rate_limited".into(),
            Self::Rejected(status) => format!("http_{status}"),
            Self::Malformed => "malformed_response".into(),
            Self::Timeout => "timeout".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProbeResult {
    pub network: String,
    pub city: String,
    pub country: Option<String>,
    pub received: f64,
    pub avg_ms: Option<f64>,
}

pub fn china_locations() -> Value {
    json!([
        {"magic":"China Telecom","limit":2},
        {"magic":"China Unicom","limit":1},
        {"magic":"China Mobile","limit":1},
    ])
}

pub fn overseas_locations(limit: u32) -> Value {
    json!([{"magic":"Europe","limit":limit}])
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
struct Measurement {
    status: String,
    #[serde(default)]
    results: Vec<ResultEntry>,
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
#[derive(Default, Deserialize)]
struct Stats {
    #[serde(default)]
    rcv: Option<f64>,
    #[serde(default)]
    avg: Option<f64>,
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
    pub fn new(transport: &'a T, token: Option<&'a str>) -> Self {
        Self {
            transport,
            token,
            poll_interval: POLL_INTERVAL,
        }
    }

    #[cfg(test)]
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
            .map_err(|_| MeasureError::Network)?;
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

    /// Runs one TCP connect measurement and waits for every probe to finish.
    pub async fn tcp_ping(
        &self,
        ip: IpAddr,
        port: u16,
        locations: Value,
    ) -> Result<Vec<ProbeResult>, MeasureError> {
        let body = json!({
            "type":"ping",
            "target":ip.to_string(),
            "locations":locations,
            "measurementOptions":{"protocol":"TCP","port":port,"packets":PACKETS},
        });
        let created = self
            .call(Method::Post, "/v1/measurements".into(), Some(body))
            .await?;
        let created: Created =
            serde_json::from_slice(&created).map_err(|_| MeasureError::Malformed)?;
        if !valid_measurement_id(&created.id) {
            return Err(MeasureError::Malformed);
        }
        for _ in 0..MAX_POLLS {
            tokio::time::sleep(self.poll_interval).await;
            let body = self
                .call(
                    Method::Get,
                    format!("/v1/measurements/{}", created.id),
                    None,
                )
                .await?;
            let measurement: Measurement =
                serde_json::from_slice(&body).map_err(|_| MeasureError::Malformed)?;
            if measurement.status == "finished" {
                return Ok(measurement
                    .results
                    .into_iter()
                    // A probe without counters reported nothing about the target.
                    .filter_map(|entry| {
                        let stats = entry.result.stats?;
                        Some(ProbeResult {
                            network: clipped(entry.probe.network),
                            city: clipped(entry.probe.city),
                            country: entry.probe.country,
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

#[cfg(test)]
pub(super) mod fake {
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
        pub fn fail(self) -> Self {
            self.replies
                .lock()
                .unwrap()
                .push_back(Err(TransportFailure));
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

    const TARGET: &str = "198.51.100.7";

    fn ip() -> IpAddr {
        TARGET.parse().unwrap()
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
            .tcp_ping(ip(), 8443, china_locations())
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
    async fn ipv6_literals_are_sent_bare_and_without_an_ip_version() {
        let transport = Scripted::default()
            .reply(202, json!({"id":"six"}))
            .reply(200, json!({"status":"finished","results":[]}));
        Client::new(&transport, None)
            .fast_polling()
            .tcp_ping("2001:db8::1".parse().unwrap(), 443, overseas_locations(2))
            .await
            .unwrap();
        let requests = transport.requests.lock().unwrap();
        let body = requests[0].body.as_ref().unwrap();
        assert_eq!(body["target"], "2001:db8::1");
        assert!(body.get("ipVersion").is_none());
        assert!(body["measurementOptions"].get("ipVersion").is_none());
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
    async fn failures_map_to_stable_codes_without_the_address() {
        async fn failure(transport: Scripted) -> MeasureError {
            Client::new(&transport, None)
                .fast_polling()
                .tcp_ping(ip(), 443, overseas_locations(1))
                .await
                .unwrap_err()
        }
        let cases = [
            (
                failure(Scripted::default().reply(429, json!({}))).await,
                "rate_limited",
            ),
            (
                failure(Scripted::default().reply(400, json!({"error":TARGET}))).await,
                "http_400",
            ),
            (failure(Scripted::default().fail()).await, "network"),
            (
                failure(Scripted::default().reply(202, json!({"id":"../limits"}))).await,
                "malformed_response",
            ),
            (
                failure(Scripted::default().reply(202, json!({"unexpected":true}))).await,
                "malformed_response",
            ),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code);
            assert!(!error.code().contains(TARGET));
        }
    }

    #[tokio::test]
    async fn polling_is_bounded_when_a_measurement_never_finishes() {
        let mut transport = Scripted::default().reply(202, json!({"id":"slow"}));
        for _ in 0..MAX_POLLS {
            transport = transport.reply(200, json!({"status":"in-progress"}));
        }
        let error = Client::new(&transport, None)
            .fast_polling()
            .tcp_ping(ip(), 443, overseas_locations(1))
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
            .tcp_ping(ip(), 443, china_locations())
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

    /// Checks the real client against the real service. Point
    /// `APSP_GLOBALPING_SMOKE_TARGET=<ip>:<port>` at an address you may probe.
    #[tokio::test]
    #[ignore = "calls the real Globalping API and spends its quota"]
    async fn real_service_answers_in_the_expected_shape() {
        let target: std::net::SocketAddr = std::env::var("APSP_GLOBALPING_SMOKE_TARGET")
            .expect("set APSP_GLOBALPING_SMOKE_TARGET")
            .parse()
            .unwrap();
        let transport = ReqwestTransport::new().unwrap();
        let client = Client::new(&transport, None);
        assert!(client.limits().await.unwrap().limit > 0);
        let china = client
            .tcp_ping(target.ip(), target.port(), china_locations())
            .await
            .unwrap();
        assert!(!china.is_empty());
        assert!(
            china
                .iter()
                .any(|probe| probe.country.as_deref() == Some("CN"))
        );
        let overseas = client
            .tcp_ping(target.ip(), target.port(), overseas_locations(2))
            .await
            .unwrap();
        assert!(overseas.iter().any(|probe| probe.received > 0.0));
    }
}

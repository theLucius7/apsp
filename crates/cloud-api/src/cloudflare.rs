//! Cloudflare's API (v4), the one client for it: where a call goes, its bearer token, query
//! pairs and JSON body, and its answer, read within the caller's limit. Callers bring the
//! HTTP client, so the timeouts are theirs.
//!
//! Cloudflare wraps every answer in an envelope (`success`, `errors`, `result`,
//! `result_info`), and a caller judges an answer one of two ways. [`Cloudflare::call`] judges
//! it like every other provider call: by the HTTP status first (see
//! [`transport::refusal`]), then by the envelope, which must report success without errors;
//! the whole envelope is returned. [`Cloudflare::result`] reads the envelope whatever the
//! status and returns its typed `result`, or Cloudflare's first reason for refusing, so that
//! its codes (81044 is "record does not exist") reach the caller.

use super::{Failure, transport};
use apsp_panel_host::http;
use reqwest::{Client, Method, RequestBuilder, Url};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

const API: &str = "https://api.cloudflare.com/client/v4/";

pub struct Cloudflare {
    client: Client,
    base: Url,
    limit: usize,
}

/// Cloudflare's reason for refusing a call.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Reason {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
}

/// Why a call judged by its envelope has no result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The call was not sent, or no answer came back.
    Unsent,
    /// The answer could not be read whole, or is not an envelope of the expected shape.
    Unreadable,
    /// Cloudflare reported failure, with its first reason when it gave one.
    Rejected(Option<Reason>),
}

#[derive(Deserialize)]
struct Envelope<T> {
    success: bool,
    #[serde(default)]
    errors: Vec<Reason>,
    result: Option<T>,
}

impl Cloudflare {
    /// The API through `client`, reading at most `limit` bytes of an answer.
    pub fn new(client: Client, limit: usize) -> Self {
        Self {
            client,
            base: Url::parse(API).expect("the API address is a URL"),
            limit,
        }
    }

    /// The same API served at `base`, such as a stand-in in tests; `None` when `base` is not
    /// a URL.
    pub fn at(client: Client, base: &str, limit: usize) -> Option<Self> {
        let mut base = Url::parse(base).ok()?;
        // Paths are joined below the base, never in place of its last segment.
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        Some(Self {
            client,
            base,
            limit,
        })
    }

    fn request(
        &self,
        method: Method,
        url: Url,
        token: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> RequestBuilder {
        let request = self
            .client
            .request(method, url)
            .bearer_auth(token)
            .query(query);
        match body {
            Some(body) => request.json(&body),
            None => request,
        }
    }

    /// One call to `path` (relative to the API), judged like every provider call: a refused
    /// status first, then an envelope that does not report success without errors
    /// (`provider_rejected`). Returns the whole envelope.
    pub async fn call(
        &self,
        method: Method,
        path: &str,
        token: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<Value, Failure> {
        let url = self
            .base
            .join(path)
            .map_err(|_| Failure::from("invalid_response"))?;
        let response = self
            .request(method, url, token, query, body)
            .send()
            .await
            .map_err(|_| Failure::from("network_error"))?;
        if let Some(failure) = transport::refusal(&response, "http_error") {
            return Err(failure);
        }
        let envelope = transport::bounded_json(response, self.limit).await?;
        if envelope["success"] != true
            || envelope
                .get("errors")
                .is_some_and(|errors| errors.as_array().is_none_or(|errors| !errors.is_empty()))
        {
            return Err("provider_rejected".into());
        }
        Ok(envelope)
    }

    /// One call to `path` (relative to the API), judged by its envelope alone, whatever the
    /// HTTP status: when Cloudflare reports success, the envelope's `result` as `T` (`None`
    /// when it has none).
    pub async fn result<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        token: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<Option<T>, Error> {
        let url = self.base.join(path).map_err(|_| Error::Unsent)?;
        let response = self
            .request(method, url, token, query, body)
            .send()
            .await
            .map_err(|_| Error::Unsent)?;
        let body = http::body(response, self.limit)
            .await
            .map_err(|_| Error::Unreadable)?;
        let envelope: Envelope<T> = serde_json::from_slice(&body).map_err(|_| Error::Unreadable)?;
        if envelope.success {
            Ok(envelope.result)
        } else {
            Err(Error::Rejected(envelope.errors.into_iter().next()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Mock, Reply};
    use serde_json::json;

    fn client() -> Client {
        transport::client().unwrap()
    }

    #[tokio::test]
    async fn calls_go_below_the_api_path_with_the_token_as_a_bearer_credential() {
        let mock = Mock::start(vec![
            Reply::ok(
                "GET /client/v4/zones/zone",
                json!({"success":true,"result":{"name":"example.test"}}),
            ),
            Reply::ok(
                "POST /client/v4/zones/zone/dns_records",
                json!({"success":true,"errors":[],"result":{"id":"record"}}),
            ),
        ])
        .await;
        // A base without a trailing slash keeps its last segment.
        let api = Cloudflare::at(client(), &format!("{}client/v4", mock.endpoint), 1024).unwrap();
        let zone: Option<Value> = api
            .result(Method::GET, "zones/zone", "TEST_ONLY_TOKEN", &[], None)
            .await
            .unwrap();
        assert_eq!(zone.unwrap()["name"], "example.test");
        let envelope = api
            .call(
                Method::POST,
                "zones/zone/dns_records",
                "TEST_ONLY_TOKEN",
                &[],
                Some(json!({"type":"TXT"})),
            )
            .await
            .unwrap();
        assert_eq!(envelope["result"]["id"], "record");
        let requests = mock.requests();
        for request in &requests {
            assert_eq!(request.headers["authorization"], "Bearer TEST_ONLY_TOKEN");
        }
        assert_eq!(requests[1].body, json!({"type":"TXT"}));
        mock.exhausted();
    }

    #[tokio::test]
    async fn a_call_is_judged_by_its_status_first_and_then_by_the_whole_envelope() {
        let refused = Reply {
            action: "GET /zones/zone".into(),
            value: json!({"success":true,"errors":[],"result":{}}),
            status: 403,
        };
        let mock = Mock::start(vec![
            refused,
            Reply::ok(
                "GET /zones/zone",
                json!({"success":false,"errors":[],"result":null}),
            ),
            Reply::ok(
                "GET /zones/zone",
                json!({"success":true,"errors":[{"code":1000,"message":"fixture"}]}),
            ),
            Reply::ok("GET /zones/zone", json!({"success":true,"errors":null})),
            Reply::ok("GET /zones/zone", json!({"success":"true"})),
        ])
        .await;
        let api = Cloudflare::at(client(), &mock.endpoint, 1024).unwrap();
        let mut codes = Vec::new();
        for _ in 0..5 {
            let failure = api
                .call(Method::GET, "zones/zone", "TEST_ONLY_TOKEN", &[], None)
                .await
                .unwrap_err();
            codes.push(failure.code);
        }
        assert_eq!(
            codes,
            [
                "authentication_failed",
                "provider_rejected",
                "provider_rejected",
                "provider_rejected",
                "provider_rejected"
            ]
        );
        mock.exhausted();
    }

    #[tokio::test]
    async fn a_result_is_read_from_the_envelope_whatever_the_status() {
        let gone = Reply {
            action: "DELETE /zones/zone/dns_records/record".into(),
            value: json!({"success":false,"errors":[{"code":81044,"message":"Record does not exist."}],"result":null}),
            status: 404,
        };
        let mock = Mock::start(vec![
            gone,
            Reply::ok(
                "DELETE /zones/zone/dns_records/record",
                json!({"success":false}),
            ),
            Reply::ok(
                "GET /zones/zone",
                json!({"success":true,"result":{"name":"example.test"}}),
            ),
            Reply::ok("GET /zones/zone", json!({"success":true})),
            Reply::ok("GET /zones/zone", json!("not an envelope")),
            Reply::ok(
                "GET /zones/zone",
                json!({"success":true,"result":{"unexpected":true}}),
            ),
        ])
        .await;
        let api = Cloudflare::at(client(), &mock.endpoint, 1024).unwrap();
        #[derive(Debug, Deserialize)]
        struct Named {
            name: String,
        }
        let delete = || {
            api.result::<Value>(
                Method::DELETE,
                "zones/zone/dns_records/record",
                "TEST_ONLY_TOKEN",
                &[],
                None,
            )
        };
        assert_eq!(
            delete().await.unwrap_err(),
            Error::Rejected(Some(Reason {
                code: 81044,
                message: "Record does not exist.".into()
            }))
        );
        assert_eq!(delete().await.unwrap_err(), Error::Rejected(None));
        let zone = || api.result::<Named>(Method::GET, "zones/zone", "TEST_ONLY_TOKEN", &[], None);
        assert_eq!(zone().await.unwrap().unwrap().name, "example.test");
        assert!(zone().await.unwrap().is_none());
        assert_eq!(zone().await.unwrap_err(), Error::Unreadable);
        assert_eq!(zone().await.unwrap_err(), Error::Unreadable);
        mock.exhausted();
        // A port nothing listens on any more.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = closed.local_addr().unwrap();
        drop(closed);
        let unreachable = Cloudflare::at(client(), &format!("http://{address}/"), 1024).unwrap();
        assert_eq!(
            unreachable
                .result::<Value>(Method::GET, "zones/zone", "TEST_ONLY_TOKEN", &[], None)
                .await
                .unwrap_err(),
            Error::Unsent
        );
    }

    #[tokio::test]
    async fn answers_beyond_the_limit_are_not_read() {
        let large = json!({"success":true,"result":"x".repeat(2048)});
        let mock = Mock::start(vec![
            Reply::ok("GET /zones/zone", large.clone()),
            Reply::ok("GET /zones/zone", large),
        ])
        .await;
        let api = Cloudflare::at(client(), &mock.endpoint, 1024).unwrap();
        assert_eq!(
            api.call(Method::GET, "zones/zone", "TEST_ONLY_TOKEN", &[], None)
                .await
                .unwrap_err()
                .code,
            "response_too_large"
        );
        assert_eq!(
            api.result::<String>(Method::GET, "zones/zone", "TEST_ONLY_TOKEN", &[], None)
                .await
                .unwrap_err(),
            Error::Unreadable
        );
        mock.exhausted();
    }
}

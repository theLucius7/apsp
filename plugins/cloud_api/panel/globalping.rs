//! Public Globalping measurements, used to judge reachability from one country.

use super::{Failure, transport};
use serde_json::{Value, json};

const ENDPOINT: &str = "https://api.globalping.io/v1/measurements";
const USER_AGENT: &str = "apsp-panel";

pub struct Globalping {
    client: reqwest::Client,
    endpoint: String,
}

/// One probe's outcome. `reached` means at least one TCP handshake completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeResult {
    pub network: String,
    pub reached: bool,
}

impl Globalping {
    pub fn new() -> Result<Self, Failure> {
        Ok(Self {
            client: transport::client()?,
            endpoint: ENDPOINT.into(),
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn local(endpoint: &str) -> Self {
        assert_eq!(
            reqwest::Url::parse(endpoint).unwrap().host_str(),
            Some("127.0.0.1")
        );
        Self {
            client: transport::client().unwrap(),
            endpoint: endpoint.trim_end_matches('/').into(),
        }
    }

    /// Starts a TCP handshake measurement and returns its identifier.
    pub async fn tcp_ping(
        &self,
        target: &str,
        port: u16,
        country: &str,
        limit: u32,
        packets: u32,
    ) -> Result<String, Failure> {
        let body = json!({
            "type":"ping",
            "target":target,
            "limit":limit,
            "locations":[{"country":country}],
            "measurementOptions":{"protocol":"TCP","port":port,"packets":packets},
        });
        let response = transport::json(
            self.client
                .post(&self.endpoint)
                .header("user-agent", USER_AGENT)
                .json(&body),
        )
        .await?;
        response["id"]
            .as_str()
            .filter(|id| identifier(id))
            .map(str::to_owned)
            .ok_or(Failure::from("invalid_response"))
    }

    /// Returns `None` while the measurement is still running.
    pub async fn read(&self, id: &str) -> Result<Option<Vec<ProbeResult>>, Failure> {
        if !identifier(id) {
            return Err("invalid_response".into());
        }
        let value = transport::json(
            self.client
                .get(format!("{}/{id}", self.endpoint))
                .header("user-agent", USER_AGENT),
        )
        .await?;
        parse(&value)
    }
}

fn identifier(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn parse(value: &Value) -> Result<Option<Vec<ProbeResult>>, Failure> {
    match value["status"].as_str() {
        Some("in-progress") => return Ok(None),
        Some("finished") => {}
        _ => return Err("invalid_response".into()),
    }
    let rows = value["results"]
        .as_array()
        .ok_or(Failure::from("invalid_response"))?;
    Ok(Some(
        rows.iter()
            .map(|row| ProbeResult {
                network: row["probe"]["network"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                reached: row["result"]["stats"]["loss"]
                    .as_f64()
                    .is_some_and(|loss| loss < 100.0),
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_measurements_stay_pending_and_missing_statistics_count_as_unreached() {
        assert!(parse(&json!({"status":"in-progress"})).unwrap().is_none());
        assert!(parse(&json!({"status":"other"})).is_err());
        assert!(parse(&json!({"status":"finished"})).is_err());
        let rows = parse(&json!({"status":"finished","results":[
            {"probe":{"network":"Example Telecom"},"result":{"stats":{"loss":0}}},
            {"probe":{"network":"Example Mobile"},"result":{"stats":{"loss":100}}},
            {"probe":{"network":"Example Unicom"},"result":{"status":"failed"}},
            {"probe":{},"result":{"stats":{"loss":66.6}}},
        ]}))
        .unwrap()
        .unwrap();
        assert_eq!(
            rows.iter().map(|row| row.reached).collect::<Vec<_>>(),
            [true, false, false, true]
        );
        assert_eq!(rows[0].network, "Example Telecom");
        assert_eq!(rows[3].network, "");
    }

    #[test]
    fn measurement_identifiers_cannot_alter_the_request_path() {
        assert!(identifier("abc123XYZ"));
        for invalid in ["", "../admin", "a/b", "a?b", &"a".repeat(65)] {
            assert!(!identifier(invalid), "{invalid}");
        }
    }
}

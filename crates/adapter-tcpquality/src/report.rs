use crate::{
    files::now_millis,
    input::{self, Target, Validated},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer};
use std::net::{IpAddr, SocketAddr};

fn nullable<'de, T, D>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Engine {
    name: String,
    version: String,
    #[serde(deserialize_with = "nullable")]
    source_commit: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    ip_version: String,
    count: u8,
    concurrency: u8,
    dns_timeout_ms: u64,
    connect_timeout_ms: u64,
    interval_ms: u64,
    total_timeout_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sample {
    attempted_at_ms: u64,
    elapsed_ms: f64,
    #[serde(deserialize_with = "nullable")]
    latency_ms: Option<f64>,
    #[serde(deserialize_with = "nullable")]
    error: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Summary {
    attempted: usize,
    succeeded: usize,
    #[serde(deserialize_with = "nullable")]
    connection_success_percent: Option<f64>,
    #[serde(deserialize_with = "nullable")]
    latency_min_ms: Option<f64>,
    #[serde(deserialize_with = "nullable")]
    latency_mean_ms: Option<f64>,
    #[serde(deserialize_with = "nullable")]
    latency_max_ms: Option<f64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TargetResult {
    target: Target,
    #[serde(deserialize_with = "nullable")]
    address: Option<String>,
    dns_attempts: u8,
    status: String,
    samples: Vec<Sample>,
    summary: Summary,
    #[serde(deserialize_with = "nullable")]
    error: Option<String>,
    pub complete: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Report {
    schema: u32,
    method: String,
    semantics: String,
    engine: Engine,
    started_at_ms: u64,
    #[serde(deserialize_with = "nullable")]
    finished_at_ms: Option<u64>,
    parameters: Parameters,
    target_digest: String,
    targets: Vec<TargetResult>,
    pub complete: bool,
    deadline_exceeded: bool,
    upload_enabled: bool,
    ranking_enabled: bool,
    speedtest_enabled: bool,
}
fn close(left: f64, right: f64) -> bool {
    left.is_finite() && right.is_finite() && (left - right).abs() <= 1e-8 + right.abs() * 1e-8
}
fn same(left: Option<f64>, right: Option<f64>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => close(left, right),
        _ => false,
    }
}
impl TargetResult {
    pub fn valid(&self, expected: &Target, input: &Validated<'_>, start: u64, end: u64) -> bool {
        if self.target != *expected
            || self.dns_attempts > 1
            || self.samples.len() > usize::from(input.count)
            || !matches!(
                self.status.as_str(),
                "not_attempted" | "resolving" | "running" | "partial" | "completed" | "failed"
            )
            || self.error.as_deref().is_some_and(|error| {
                !matches!(
                    error,
                    "dns_error"
                        | "dns_timeout"
                        | "ip_family_unavailable"
                        | "total_timeout"
                        | "worker_failed"
                        | "utc_unavailable"
                )
            })
        {
            return false;
        }
        let literal = expected.target.parse::<IpAddr>().ok();
        if literal.is_some() && self.dns_attempts != 0 {
            return false;
        }
        let address = match self.address.as_deref() {
            Some(value) => match value.parse::<SocketAddr>() {
                Ok(value) => Some(value),
                Err(_) => return false,
            },
            None => None,
        };
        if let Some(address) = address {
            if !input::unicast(address.ip())
                || address.port() != expected.port
                || (address.is_ipv4() != (input.ip_version == "4"))
                || literal.is_some_and(|ip| ip != address.ip())
                || (literal.is_none() && self.dns_attempts != 1)
            {
                return false;
            }
        } else if !self.samples.is_empty() {
            return false;
        }
        if self.status == "not_attempted"
            && (self.dns_attempts != 0
                || address.is_some()
                || !self.samples.is_empty()
                || self.complete)
        {
            return false;
        }
        if self.complete && !matches!(self.status.as_str(), "completed" | "failed") {
            return false;
        }
        if self.status == "completed"
            && (!self.complete
                || self.samples.len() != usize::from(input.count)
                || self.error.is_some())
        {
            return false;
        }
        if self.status == "failed" {
            if self.complete {
                if !matches!(
                    self.error.as_deref(),
                    Some("dns_error" | "dns_timeout" | "ip_family_unavailable")
                ) || !self.samples.is_empty()
                    || address.is_some()
                {
                    return false;
                }
                if matches!(self.error.as_deref(), Some("dns_error" | "dns_timeout"))
                    && (literal.is_some() || self.dns_attempts != 1)
                {
                    return false;
                }
                if self.error.as_deref() == Some("ip_family_unavailable")
                    && (literal.is_some_and(|ip| ip.is_ipv4() == (input.ip_version == "4"))
                        || (literal.is_none() && self.dns_attempts != 1))
                {
                    return false;
                }
            } else if self.error.as_deref() != Some("utc_unavailable") {
                return false;
            }
        }
        let mut previous = start;
        for sample in &self.samples {
            if sample.attempted_at_ms < previous
                || sample.attempted_at_ms > end
                || !sample.elapsed_ms.is_finite()
                || !(0.0..=60_000.0).contains(&sample.elapsed_ms)
                || sample.error.as_deref().is_some_and(|error| {
                    !matches!(
                        error,
                        "connect_timeout" | "connect_refused" | "connect_error"
                    )
                })
                || !same(
                    sample.latency_ms,
                    sample.error.is_none().then_some(sample.elapsed_ms),
                )
            {
                return false;
            }
            previous = sample.attempted_at_ms;
        }
        let latencies: Vec<f64> = self
            .samples
            .iter()
            .filter_map(|sample| sample.latency_ms)
            .collect();
        let succeeded = latencies.len();
        let rate = (!self.samples.is_empty())
            .then(|| succeeded as f64 / self.samples.len() as f64 * 100.0);
        self.summary.attempted == self.samples.len()
            && self.summary.succeeded == succeeded
            && same(self.summary.connection_success_percent, rate)
            && same(
                self.summary.latency_min_ms,
                latencies.iter().copied().reduce(f64::min),
            )
            && same(
                self.summary.latency_max_ms,
                latencies.iter().copied().reduce(f64::max),
            )
            && same(
                self.summary.latency_mean_ms,
                (!latencies.is_empty()).then(|| latencies.iter().sum::<f64>() / succeeded as f64),
            )
    }
}
impl Report {
    pub fn parse(text: &str, input: &Validated<'_>) -> Result<Self> {
        let report: Self = serde_json::from_str(text).context("invalid TCP report JSON")?;
        let latest = now_millis()?.saturating_add(5_000);
        ensure!(
            report.schema == 1
                && report.method == "tcp_connect"
                && report.semantics == input::SEMANTICS
                && report.engine.name == "apsp-native-tcp-connect-v1"
                && report.engine.version == input::ENGINE_VERSION
                && report.engine.source_commit.as_deref() == Some(input.source)
                && report.target_digest == input.digest
                && !report.upload_enabled
                && !report.ranking_enabled
                && !report.speedtest_enabled,
            "TCP report identity or safety scope mismatch"
        );
        let params = &report.parameters;
        ensure!(
            params.ip_version == input.ip_version
                && params.count == input.count
                && params.concurrency == input.concurrency
                && params.dns_timeout_ms == 2_000
                && params.connect_timeout_ms == 1_000
                && params.interval_ms == 250
                && params.total_timeout_ms == 60_000,
            "TCP report parameters mismatch"
        );
        ensure!(
            report.started_at_ms > 0
                && report.started_at_ms <= latest
                && report
                    .finished_at_ms
                    .is_none_or(|end| end >= report.started_at_ms && end <= latest)
                && (!report.complete || report.finished_at_ms.is_some())
                && !(report.complete && report.deadline_exceeded)
                && (!report.deadline_exceeded || report.finished_at_ms.is_some()),
            "TCP report UTC or completion mismatch"
        );
        let end = report.finished_at_ms.unwrap_or(latest);
        ensure!(
            report.targets.len() == input.snapshot.targets.len()
                && report
                    .targets
                    .iter()
                    .zip(&input.snapshot.targets)
                    .all(|(result, expected)| result.valid(
                        expected,
                        input,
                        report.started_at_ms,
                        end
                    ))
                && (!report.complete || report.targets.iter().all(|result| result.complete)),
            "TCP report targets or statistics mismatch"
        );
        Ok(report)
    }
}
pub(crate) fn target(text: &str, expected: &Target, input: &Validated<'_>) -> Result<TargetResult> {
    let target: TargetResult = serde_json::from_str(text).context("invalid TCP target chapter")?;
    ensure!(
        target.valid(expected, input, 1, now_millis()?.saturating_add(5_000)),
        "TCP target chapter scope or statistics mismatch"
    );
    Ok(target)
}

use crate::{ExecutionContext, SOURCE_COMMIT, SOURCE_SHA256, VERSION, files};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::{collections::BTreeSet, net::IpAddr};

fn nullable<'de, T, D>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    seq: u32,
    provider: String,
    dataset: String,
    #[serde(deserialize_with = "nullable")]
    target_ip: Option<String>,
    #[serde(deserialize_with = "nullable")]
    url: Option<String>,
    status: String,
    #[serde(deserialize_with = "nullable")]
    attempted_at: Option<i64>,
    #[serde(deserialize_with = "nullable")]
    elapsed_ms: Option<u64>,
    #[serde(deserialize_with = "nullable")]
    http_status: Option<u16>,
    #[serde(deserialize_with = "nullable")]
    curl_exit: Option<u16>,
    #[serde(deserialize_with = "nullable")]
    response_bytes: Option<u64>,
    #[serde(deserialize_with = "nullable")]
    error_kind: Option<String>,
    #[serde(deserialize_with = "nullable")]
    error_message: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Report {
    schema: u32,
    // Preserve the signed report format produced by the installed diagnostic tool.
    #[serde(rename = "plugin")]
    tool: String,
    version: String,
    job_id: String,
    ip_version: String,
    artifact_sha256: String,
    source_commit: String,
    source_sha256: String,
    pub(crate) started_at: i64,
    #[serde(deserialize_with = "nullable")]
    pub finished_at: Option<i64>,
    #[serde(deserialize_with = "nullable")]
    egress_ip: Option<String>,
    #[serde(deserialize_with = "nullable")]
    upstream: Option<Value>,
    attempts: Vec<Attempt>,
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !matches!(a, 0 | 10 | 127 | 224..=255)
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 169 && b == 254)
                && !(a == 172 && (16..=31).contains(&b))
                && !(a == 192 && ((b == 0 && matches!(c, 0 | 2)) || b == 168))
                && !(a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
                && !(a == 203 && b == 0 && c == 113)
        }
        IpAddr::V6(ip) => {
            let words = ip.segments();
            words[0] & 0xe000 == 0x2000 && !(words[0] == 0x2001 && words[1] == 0x0db8)
        }
    }
}

fn address(value: &str, family: &str) -> Result<IpAddr> {
    let ip: IpAddr = value.parse().context("invalid IPQuality address")?;
    ensure!(
        public_ip(ip) && ip.is_ipv4() == (family == "4") && ip.to_string() == value,
        "IPQuality address must be canonical, public, and match its requested family"
    );
    Ok(ip)
}

fn source(provider: &str, dataset: &str) -> bool {
    match provider {
        "egress-discovery" => dataset == "egress",
        "check-place-aggregator" => matches!(
            dataset,
            "MaxMind" | "SCAMALYTICS" | "ipapi" | "AbuseIPDB" | "IP2LOCATION" | "ipdata" | "IPQS"
        ),
        "ipinfo-public-widget" => dataset == "IPinfo",
        "netflix-public-pages" => dataset == "Netflix",
        "youtube-public-page" => dataset == "Youtube",
        "tiktok-public-page" => dataset == "TikTok",
        "primevideo-public-page" => dataset == "AmazonPrimeVideo",
        "reddit-public-endpoint" => dataset == "Reddit",
        "ipregistry-not-configured" => dataset == "ipregistry",
        "dbip-not-configured" => dataset == "DBIP",
        "disney-not-configured" => dataset == "DisneyPlus",
        "openai-not-configured" => dataset == "OpenAI",
        "smtp-disabled" => dataset == "SMTP",
        "dnsbl-disabled" => dataset == "DNSBL",
        _ => false,
    }
}

fn url(value: &str, provider: &str) -> bool {
    if value.len() > 2048
        || value
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return false;
    }
    let Some(rest) = value.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or_default();
    if host.contains('@') || value.contains('#') {
        return false;
    }
    match provider {
        "egress-discovery" => matches!(host, "api64.ipify.org" | "ident.me"),
        "check-place-aggregator" => host == "ipinfo.check.place",
        "ipinfo-public-widget" => host == "ipinfo.io",
        "netflix-public-pages" => host == "www.netflix.com",
        "youtube-public-page" => host == "www.youtube.com",
        "tiktok-public-page" => host == "www.tiktok.com",
        "primevideo-public-page" => host == "www.primevideo.com",
        "reddit-public-endpoint" => host == "www.reddit.com",
        _ => false,
    }
}

impl Report {
    pub(crate) fn validate_section_time(&self, collected_at: i64, complete: bool) -> Result<()> {
        ensure!(
            collected_at >= self.started_at
                && self.finished_at.is_none_or(|end| end <= collected_at)
                && self
                    .attempts
                    .iter()
                    .all(|attempt| attempt.attempted_at.is_none_or(|at| at <= collected_at))
                && (!complete || self.finished_at.is_some()),
            "IPQuality chapter time or completion differs from its execution evidence"
        );
        Ok(())
    }

    pub(crate) fn parse(text: &str, context: &ExecutionContext, timeout_secs: u32) -> Result<Self> {
        let report: Self = serde_json::from_str(text).context("invalid IPQuality JSON envelope")?;
        let now = files::now()?;
        ensure!(
            report.schema == 1
                && report.tool == "ipquality"
                && report.version == VERSION
                && report.job_id == context.job_id
                && report.ip_version == context.ip_version
                && report.artifact_sha256 == context.artifact_sha256
                && report.source_commit == SOURCE_COMMIT
                && report.source_sha256 == SOURCE_SHA256,
            "IPQuality output differs from its authenticated task, fixed source, or signed artifact"
        );
        ensure!(
            report.started_at > 0
                && report.started_at <= now.saturating_add(5)
                && report.finished_at.is_none_or(|end| end >= report.started_at
                    && end <= now.saturating_add(5)
                    && end.saturating_sub(report.started_at)
                        <= i64::from(timeout_secs).saturating_add(60))
                && report.attempts.len() <= 64,
            "invalid IPQuality execution window or request count"
        );
        let end = report.finished_at.unwrap_or(now.saturating_add(5));
        let observed = report
            .egress_ip
            .as_deref()
            .map(|value| address(value, &report.ip_version))
            .transpose()?;
        if let Some(upstream) = &report.upstream {
            ensure!(
                observed.is_some() && upstream.is_object(),
                "IPQuality upstream output requires an observed public egress"
            );
            let head = upstream
                .get("Head")
                .context("IPQuality JSON is missing Head")?;
            ensure!(
                head.is_object()
                    && head.get("Version").and_then(Value::as_str) == Some("v2026-09-16"),
                "IPQuality JSON version differs from its fixed source"
            );
            let ip = head
                .get("IP")
                .and_then(Value::as_str)
                .context("IPQuality JSON is missing its unmasked target IP")?;
            ensure!(
                Some(address(ip, &report.ip_version)?) == observed,
                "IPQuality JSON target differs from the observed egress"
            );
        }
        let mut requests = BTreeSet::new();
        for (offset, attempt) in report.attempts.iter().enumerate() {
            ensure!(
                attempt.seq as usize == offset + 1 && source(&attempt.provider, &attempt.dataset),
                "IPQuality request ordering or registered source is invalid"
            );
            let target = attempt
                .target_ip
                .as_deref()
                .map(|value| address(value, &report.ip_version))
                .transpose()?;
            ensure!(
                target.is_none_or(|ip| Some(ip) == observed),
                "IPQuality request target differs from the observed egress"
            );
            if attempt.status == "not_attempted" {
                ensure!(
                    attempt.url.is_none()
                        && attempt.attempted_at.is_none()
                        && attempt.elapsed_ms.is_none()
                        && attempt.http_status.is_none()
                        && attempt.curl_exit.is_none()
                        && attempt.response_bytes.is_none()
                        && attempt.error_kind.as_deref() == Some("not_attempted")
                        && attempt.error_message.as_deref().is_some_and(valid_error),
                    "unattempted IPQuality source must remain unknown"
                );
                continue;
            }
            ensure!(
                attempt
                    .url
                    .as_deref()
                    .is_some_and(|value| url(value, &attempt.provider))
                    && attempt
                        .attempted_at
                        .is_some_and(|at| at >= report.started_at && at <= end)
                    && attempt.elapsed_ms.is_some_and(|ms| ms <= 300_000)
                    && attempt.curl_exit.is_some_and(|exit| exit <= 255)
                    && attempt
                        .response_bytes
                        .is_some_and(|bytes| bytes <= 2 * 1024 * 1024)
                    && attempt
                        .http_status
                        .is_none_or(|status| (100..=599).contains(&status)),
                "IPQuality attempted source requires bounded request evidence"
            );
            ensure!(
                requests.insert((
                    attempt.provider.as_str(),
                    attempt.url.as_deref().unwrap_or_default()
                )),
                "IPQuality may not retry the same source request"
            );
            if attempt.provider != "egress-discovery" {
                ensure!(
                    target.is_some(),
                    "IPQuality source query requires its observed target IP"
                );
            }
            match attempt.status.as_str() {
                "succeeded" => ensure!(
                    observed.is_some()
                        && target == observed
                        && attempt.http_status == Some(200)
                        && attempt.curl_exit == Some(0)
                        && attempt.response_bytes.is_some_and(|bytes| bytes > 0)
                        && attempt.error_kind.is_none()
                        && attempt.error_message.is_none(),
                    "IPQuality successful source lacks successful evidence"
                ),
                "failed" => {
                    let kind = attempt
                        .error_kind
                        .as_deref()
                        .context("failed IPQuality source is missing its error classification")?;
                    ensure!(
                        matches!(
                            kind,
                            "dns"
                                | "connect"
                                | "tls"
                                | "timeout"
                                | "http_403"
                                | "http_429"
                                | "http_other"
                                | "non_json"
                                | "schema_mismatch"
                                | "response_limit"
                                | "request_error"
                        ) && attempt.error_message.as_deref().is_some_and(valid_error)
                            && (kind != "http_403" || attempt.http_status == Some(403))
                            && (kind != "http_429" || attempt.http_status == Some(429))
                            && (attempt.http_status != Some(403) || kind == "http_403")
                            && (attempt.http_status != Some(429) || kind == "http_429"),
                        "IPQuality failed source has inconsistent error evidence"
                    );
                }
                _ => anyhow::bail!("unsupported IPQuality source state"),
            }
        }
        ensure!(
            observed.is_none()
                || report
                    .attempts
                    .iter()
                    .any(|attempt| attempt.provider == "egress-discovery"
                        && attempt.status == "succeeded"),
            "IPQuality observed egress lacks successful node discovery evidence"
        );
        Ok(report)
    }
}

fn valid_error(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 1024 && !value.chars().any(char::is_control)
}

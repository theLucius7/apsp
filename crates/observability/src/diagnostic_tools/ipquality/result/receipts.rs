use super::{Envelope, invalid, required_option};
use crate::{error::ApiResult, ip_quality::QueryErrorKind, net::public_ip};
use serde::Deserialize;
use std::net::IpAddr;

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    Succeeded,
    Failed,
    NotAttempted,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Attempt {
    pub(super) seq: u32,
    pub(super) provider: String,
    pub(super) dataset: String,
    #[serde(deserialize_with = "required_option")]
    pub(super) target_ip: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub(super) url: Option<String>,
    pub(super) status: Status,
    #[serde(deserialize_with = "required_option")]
    pub(super) attempted_at: Option<i64>,
    #[serde(deserialize_with = "required_option")]
    pub(super) elapsed_ms: Option<u64>,
    #[serde(deserialize_with = "required_option")]
    pub(super) http_status: Option<u16>,
    #[serde(deserialize_with = "required_option")]
    pub(super) curl_exit: Option<u16>,
    #[serde(deserialize_with = "required_option")]
    pub(super) response_bytes: Option<u64>,
    #[serde(deserialize_with = "required_option")]
    pub(super) error_kind: Option<QueryErrorKind>,
    #[serde(deserialize_with = "required_option")]
    pub(super) error_message: Option<String>,
}

pub(crate) const SOURCES: &[(&str, &[&str])] = &[
    (
        "check-place-aggregator",
        &[
            "MaxMind",
            "SCAMALYTICS",
            "ipapi",
            "AbuseIPDB",
            "IP2LOCATION",
            "ipdata",
            "IPQS",
        ],
    ),
    ("ipinfo-public-widget", &["IPinfo"]),
    ("netflix-public-pages", &["Netflix"]),
    ("youtube-public-page", &["Youtube"]),
    ("tiktok-public-page", &["TikTok"]),
    ("primevideo-public-page", &["AmazonPrimeVideo"]),
    ("reddit-public-endpoint", &["Reddit"]),
    ("egress-discovery", &["egress"]),
    ("ipregistry-not-configured", &["ipregistry"]),
    ("dbip-not-configured", &["DBIP"]),
    ("disney-not-configured", &["DisneyPlus"]),
    ("openai-not-configured", &["OpenAI"]),
    ("smtp-disabled", &["SMTP"]),
    ("dnsbl-disabled", &["DNSBL"]),
];

pub(super) fn canonical(value: &str, family: &str) -> Option<String> {
    let ip: IpAddr = value.parse().ok()?;
    (public_ip(ip) && ((family == "4") == ip.is_ipv4())).then(|| ip.to_string())
}

fn allowed_pair(provider: &str, dataset: &str) -> bool {
    SOURCES
        .iter()
        .any(|(source, datasets)| *source == provider && datasets.contains(&dataset))
}

pub(super) fn bounded_text(text: &str, limit: usize) -> bool {
    !text.trim().is_empty() && text.len() <= limit && !text.chars().any(char::is_control)
}

fn allowed_host(provider: &str, host: &str) -> bool {
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

pub(super) fn validate_attempt(
    attempt: &Attempt,
    envelope: &Envelope,
    collected_at: i64,
    expires_at: i64,
) -> ApiResult<()> {
    if !allowed_pair(&attempt.provider, &attempt.dataset)
        || attempt
            .error_message
            .as_ref()
            .is_some_and(|message| !bounded_text(message, 1024))
        || attempt.elapsed_ms.is_some_and(|elapsed| elapsed > 300_000)
        || attempt
            .response_bytes
            .is_some_and(|bytes| bytes > 2 * 1024 * 1024)
        || attempt
            .http_status
            .is_some_and(|status| !(100..=599).contains(&status))
        || attempt.curl_exit.is_some_and(|code| code > 255)
        || attempt.error_kind.is_some_and(|kind| {
            !matches!(
                kind,
                QueryErrorKind::Dns
                    | QueryErrorKind::Connect
                    | QueryErrorKind::Tls
                    | QueryErrorKind::Timeout
                    | QueryErrorKind::Http403
                    | QueryErrorKind::Http429
                    | QueryErrorKind::HttpOther
                    | QueryErrorKind::NonJson
                    | QueryErrorKind::SchemaMismatch
                    | QueryErrorKind::ResponseLimit
                    | QueryErrorKind::RequestError
                    | QueryErrorKind::NotAttempted
            )
        })
    {
        return Err(invalid("逐源记录的来源、类型或界限不正确"));
    }
    if let Some(target) = &attempt.target_ip
        && (canonical(target, &envelope.ip_version).as_deref() != Some(target.as_str())
            || envelope.egress_ip.as_deref() != Some(target.as_str()))
    {
        return Err(invalid("逐源目标与本次出口 IP 不一致"));
    }
    if attempt.status == Status::NotAttempted {
        if attempt.attempted_at.is_some()
            || attempt.elapsed_ms.is_some()
            || attempt.http_status.is_some()
            || attempt.curl_exit.is_some()
            || attempt.response_bytes.is_some()
            || attempt.url.is_some()
            || attempt.error_kind != Some(QueryErrorKind::NotAttempted)
        {
            return Err(invalid("未查询来源不能声明请求收据"));
        }
        return Ok(());
    }
    if attempt.provider != "egress-discovery" && attempt.target_ip.is_none() {
        return Err(invalid("已查询来源必须绑定本次出口目标"));
    }
    let Some(at) = attempt.attempted_at else {
        return Err(invalid("已查询来源没有真实尝试时间"));
    };
    if at < envelope.started_at
        || at > envelope.finished_at.unwrap_or(collected_at)
        || at > expires_at
        || attempt.elapsed_ms.is_none()
        || attempt.curl_exit.is_none()
    {
        return Err(invalid("逐源请求不在本次执行时间内"));
    }
    let Some(url) = attempt
        .url
        .as_ref()
        .and_then(|url| reqwest::Url::parse(url).ok())
    else {
        return Err(invalid("已查询来源没有有效目标地址"));
    };
    if url.as_str().len() > 2048
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port().is_some()
        || !url
            .host_str()
            .is_some_and(|host| allowed_host(&attempt.provider, host))
        || attempt
            .url
            .as_ref()
            .is_some_and(|url| url.chars().any(char::is_whitespace))
    {
        return Err(invalid("逐源请求地址不属于 HTTPS 公开接口"));
    }
    if attempt.status == Status::Succeeded {
        if attempt.target_ip.is_none()
            || envelope.egress_ip.is_none()
            || attempt.curl_exit != Some(0)
            || attempt.http_status != Some(200)
            || !attempt.response_bytes.is_some_and(|bytes| bytes > 0)
            || attempt.error_kind.is_some()
            || attempt.error_message.is_some()
        {
            return Err(invalid("成功来源没有完整的目标、传输及响应收据"));
        }
    } else if attempt.error_kind.is_none()
        || attempt.error_kind == Some(QueryErrorKind::NotAttempted)
        || attempt.error_message.is_none()
    {
        return Err(invalid("失败来源没有可用的错误分类"));
    }
    if (attempt.http_status == Some(403)) != (attempt.error_kind == Some(QueryErrorKind::Http403))
        || (attempt.http_status == Some(429))
            != (attempt.error_kind == Some(QueryErrorKind::Http429))
    {
        return Err(invalid("访问被拒绝或限流的状态与错误分类不一致"));
    }
    Ok(())
}

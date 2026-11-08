use super::*;
use crate::ip_quality::{IpQuality, QualityDatabase, QueryErrorKind, QueryFailure};
use std::{collections::BTreeSet, net::IpAddr};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeReport {
    schema: String,
    job_id: uuid::Uuid,
    execution: String,
    ip_version: String,
    started_at: i64,
    finished_at: i64,
    ips: Vec<String>,
    results: Vec<SourceResult>,
    streaming: Streaming,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Streaming {
    execution: String,
    status: String,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceResult {
    provider: String,
    database: String,
    target_ip: String,
    execution: String,
    source: String,
    available: bool,
    observed_ip: Option<String>,
    attempted_at: Option<i64>,
    elapsed_ms: Option<u64>,
    data: Option<Value>,
    error: Option<SourceError>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceError {
    kind: QueryErrorKind,
    message: String,
    http_status: Option<u16>,
}

fn invalid() -> ApiError {
    ApiError::BadRequest("节点 IP 章节不符合冻结任务、正式来源或执行时间契约".into())
}
fn public(value: &str) -> bool {
    value
        .parse::<IpAddr>()
        .is_ok_and(|ip| public_node_ip(ip) && ip.to_string() == value)
}
fn bounded_text(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn contract(provider: &str) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    match provider {
        "ipregistry-node" => Some((
            "ipregistry-v1",
            "Ipregistry 正式节点查询",
            "https://api.ipregistry.co",
            "ip",
        )),
        "dbip-node" => Some((
            "dbip-v2",
            "DB-IP 正式节点查询",
            "https://api.db-ip.com/v2",
            "ipAddress",
        )),
        _ => None,
    }
}

fn allowed_path(database: &str, path: &str) -> bool {
    match database {
        "ipregistry-v1" => matches!(
            path,
            "ip" | "type"
                | "connection.asn"
                | "connection.organization"
                | "connection.type"
                | "location.country.code"
                | "security.is_proxy"
                | "security.is_tor"
                | "security.is_vpn"
                | "security.is_abuser"
                | "security.is_attacker"
                | "security.is_cloud_provider"
        ),
        "dbip-v2" => matches!(
            path,
            "ipAddress"
                | "countryCode"
                | "countryName"
                | "asNumber"
                | "asName"
                | "isp"
                | "usageType"
                | "isProxy"
                | "isCrawler"
                | "latitude"
                | "longitude"
        ),
        _ => false,
    }
}
fn safe_data(database: &str, value: &Value, prefix: &str) -> bool {
    if let Some(object) = value.as_object() {
        !object.is_empty()
            && object.iter().all(|(key, value)| {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                safe_data(database, value, &path)
            })
    } else {
        allowed_path(database, prefix)
            && match value {
                Value::String(value) => value.len() <= 512 && !value.chars().any(char::is_control),
                Value::Number(_) | Value::Bool(_) => true,
                _ => false,
            }
    }
}

fn scalar_count(value: &Value) -> usize {
    value
        .as_object()
        .map_or(1, |object| object.values().map(scalar_count).sum())
}

fn declared_types(value: &Value, prefix: &str) -> bool {
    if let Some(object) = value.as_object() {
        object.iter().all(|(key, value)| {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            declared_types(value, &path)
        })
    } else {
        match prefix {
            "security.is_proxy"
            | "security.is_tor"
            | "security.is_vpn"
            | "security.is_abuser"
            | "security.is_attacker"
            | "security.is_cloud_provider"
            | "isProxy"
            | "isCrawler" => value.is_boolean(),
            "connection.asn" | "asNumber" | "latitude" | "longitude" => value.is_number(),
            _ => value.is_string(),
        }
    }
}

fn failure_message(kind: QueryErrorKind, status: Option<u16>) -> String {
    let text = match kind {
        QueryErrorKind::Http403 => "节点正式接口拒绝访问（HTTP 403）",
        QueryErrorKind::Http429 => "节点正式接口限制请求频率（HTTP 429）",
        QueryErrorKind::HttpOther => "节点正式接口返回非成功 HTTP 状态",
        QueryErrorKind::Dns => "节点正式接口 DNS 解析失败",
        QueryErrorKind::Connect => "节点无法连接正式接口",
        QueryErrorKind::Tls => "节点正式接口 TLS 验证或握手失败",
        QueryErrorKind::Timeout => "节点正式接口查询超时",
        QueryErrorKind::NotAttempted => "节点尚未执行该来源或该 IP 版本的正式查询",
        QueryErrorKind::SchemaMismatch => "正式接口未确认有效字段、IP 身份或本次节点出口",
        QueryErrorKind::ResponseLimit => "节点正式接口响应超过上限",
        QueryErrorKind::NonJson => "节点正式接口没有返回有效 JSON",
        QueryErrorKind::BodyError => "节点正式接口响应读取失败",
        _ => "节点正式接口请求未完成",
    };
    if kind == QueryErrorKind::HttpOther {
        format!("{text}（HTTP {}），信息未知", status.unwrap_or(0))
    } else {
        format!("{text}，信息未知")
    }
}

pub(super) fn parse(
    job: &Value,
    update: &DiagnosticSectionUpdate,
    created_at: i64,
    expires_at: i64,
) -> ApiResult<Vec<IpQuality>> {
    let report: NodeReport = serde_json::from_str(&update.text).map_err(|_| invalid())?;
    let frozen: Vec<String> =
        serde_json::from_str(job["options"]["node_ips"].as_str().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
    if job["plugin"].as_str() != Some("nodequality")
        || job["version"].as_str() != Some(NODE_QUERY_VERSION)
        || job["id"].as_str() != Some(update.id.to_string().as_str())
        || report.job_id != update.id
        || report.schema != SCHEMA
        || report.execution != "node"
        || report.ips != frozen
        || !matches!(report.ip_version.as_str(), "both" | "ipv4" | "ipv6")
        || job["options"]["ip_version"].as_str() != Some(report.ip_version.as_str())
        || frozen.is_empty()
        || frozen.len() > 8
        || frozen.iter().any(|ip| !public(ip))
        || frozen.iter().collect::<BTreeSet<_>>().len() != frozen.len()
        || report.results.len() != frozen.len() * 2
        || !update.valid()
        || report.started_at < created_at.saturating_sub(300)
        || report.started_at > report.finished_at
        || report.finished_at > expires_at.saturating_add(300)
        || report.finished_at > now_timestamp().saturating_add(300)
        || report.finished_at > update.collected_at
        || report.finished_at - report.started_at > 90
        || report.streaming.execution != "node"
        || report.streaming.status != "unknown"
        || !bounded_text(&report.streaming.reason, 1024)
    {
        return Err(invalid());
    }
    let mut seen = BTreeSet::new();
    let mut output = Vec::with_capacity(report.results.len());
    for row in report.results {
        let (database, label, source, identity) = contract(&row.provider).ok_or_else(invalid)?;
        let selected_family = report.ip_version == "both"
            || (report.ip_version == "ipv6") == row.target_ip.contains(':');
        if row.database != database
            || row.source != source
            || row.execution != "node"
            || !frozen.contains(&row.target_ip)
            || !seen.insert((row.provider.clone(), row.target_ip.clone()))
            || row
                .observed_ip
                .as_ref()
                .is_some_and(|ip| !public(ip) || ip.contains(':') != row.target_ip.contains(':'))
            || row.attempted_at.is_some() != row.elapsed_ms.is_some()
            || row.elapsed_ms.is_some_and(|ms| ms > 75_000)
            || row.attempted_at.is_some_and(|at| {
                at < report.started_at || at > report.finished_at || !selected_family
            })
            || row
                .attempted_at
                .zip(row.elapsed_ms)
                .is_some_and(|(at, ms)| at.saturating_add((ms / 1000) as i64) > report.finished_at)
        {
            return Err(invalid());
        }
        let mut fields = Vec::new();
        let mut error = row.error;
        if let Some(data) = row.data.as_ref() {
            if error.is_some()
                || !row.available
                || row.attempted_at.is_none()
                || row.observed_ip.as_deref() != Some(row.target_ip.as_str())
                || !safe_data(database, data, "")
                || data[identity].as_str() != Some(row.target_ip.as_str())
                || (database == "ipregistry-v1"
                    && data["type"].as_str()
                        != Some(if row.target_ip.contains(':') {
                            "IPv6"
                        } else {
                            "IPv4"
                        }))
            {
                return Err(invalid());
            }
            fields = ip_quality::confirmed_node_fields(database, data);
            let identity_count = if database == "ipregistry-v1" { 2 } else { 1 };
            if fields.is_empty()
                || !declared_types(data, "")
                || fields.len() + identity_count != scalar_count(data)
            {
                fields.clear();
                error = Some(SourceError {
                    kind: QueryErrorKind::SchemaMismatch,
                    message: "没有有效字段".into(),
                    http_status: None,
                });
            }
        } else if error.is_none() {
            return Err(invalid());
        }
        if let Some(error) = &error
            && (!bounded_text(&error.message, 1024)
                || error
                    .http_status
                    .is_some_and(|status| !(300..=599).contains(&status))
                || match error.kind {
                    QueryErrorKind::Http403 => error.http_status != Some(403),
                    QueryErrorKind::Http429 => error.http_status != Some(429),
                    QueryErrorKind::HttpOther => error
                        .http_status
                        .is_none_or(|status| matches!(status, 403 | 429)),
                    _ => error.http_status.is_some(),
                }
                || !row.available && error.kind != QueryErrorKind::NotAttempted
                || row.attempted_at.is_none() && error.kind != QueryErrorKind::NotAttempted)
        {
            return Err(invalid());
        }
        let failure = error.as_ref().map(|error| QueryFailure {
            kind: Some(error.kind),
            message: failure_message(error.kind, error.http_status),
            http_status: error.http_status,
            attempted_at: row.attempted_at,
            elapsed_ms: row.elapsed_ms,
        });
        let success_at = (!fields.is_empty()).then(|| {
            row.attempted_at
                .unwrap_or(report.finished_at)
                .saturating_add((row.elapsed_ms.unwrap_or(0) / 1000) as i64)
        });
        let fresh_until = success_at.map(|at| at.saturating_add(86400));
        let dataset = QualityDatabase {
            database: database.into(),
            label: label.into(),
            status: String::from(if success_at.is_some() {
                "succeeded"
            } else {
                "failed"
            }),
            fields,
            error: failure.as_ref().map(|failure| failure.message.clone()),
            provider: row.provider.clone(),
            target_ip: Some(row.target_ip.clone()),
            attempted_at: row.attempted_at,
            elapsed_ms: row.elapsed_ms,
            error_kind: failure.as_ref().and_then(|failure| failure.kind),
            http_status: failure.as_ref().and_then(|failure| failure.http_status),
            last_attempt_at: row.attempted_at,
            last_success_at: success_at,
            fresh_until,
            last_error: failure.clone(),
            historical: false,
            available: Some(row.available),
            unavailable_reason: (!row.available)
                .then(|| "节点未配置可安全读取的正式私有凭证与操作授权；信息未知".into()),
            execution: "node".into(),
            observed_ip: row.observed_ip,
            source: Some(source.into()),
        };
        output.push(IpQuality {
            ip: row.target_ip,
            checked_at: report.finished_at,
            expires_at: fresh_until.unwrap_or(0),
            status: dataset.status.clone(),
            provider: row.provider,
            databases: vec![dataset],
            last_attempt_at: row.attempted_at,
            last_success_at: success_at,
            fresh_until,
            last_error: failure
                .map(|error| BTreeMap::from([(database.into(), error)]))
                .unwrap_or_default(),
        });
    }
    Ok(output)
}

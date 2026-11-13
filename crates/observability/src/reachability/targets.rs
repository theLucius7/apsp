//! Which endpoints to measure, and the REALITY risk flags shown beside them.

use crate::error::ApiResult;
use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NodeInfo {
    pub kind: &'static str,
    pub id: i64,
    pub name: String,
    pub server_id: i64,
    pub server_name: String,
    pub protocol: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct RiskFlag {
    pub code: &'static str,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub nodes: Vec<NodeInfo>,
    /// False when every node here speaks UDP, where a TCP check proves nothing.
    pub tcp: bool,
    pub risks: Vec<RiskFlag>,
}

impl Target {
    pub fn node_names(&self) -> String {
        self.nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>()
            .join("、")
    }
    pub fn server_names(&self) -> String {
        let names: BTreeSet<_> = self
            .nodes
            .iter()
            .map(|node| node.server_name.as_str())
            .collect();
        names.into_iter().collect::<Vec<_>>().join("、")
    }
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub info: NodeInfo,
    pub host: String,
    pub port: u16,
    pub tcp: bool,
    /// Present for REALITY nodes: the SNI and target names known without decrypting.
    pub reality_names: Option<Vec<String>>,
}

/// Canonical form used for deduplication and for measuring.
pub fn normalize_host(raw: &str) -> Option<String> {
    let host = raw
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.');
    if !crate::text::Text::bytes(253).not_empty().accepts(host)
        || host.contains(char::is_whitespace)
    {
        return None;
    }
    Some(match host.parse::<IpAddr>() {
        Ok(ip) => ip.to_string(),
        Err(_) => host.to_ascii_lowercase(),
    })
}

/// Imported public ports may be a hopping range; the first port stands for it.
pub fn first_port(value: &Value) -> Option<u16> {
    let port = match value {
        Value::Number(number) => u16::try_from(number.as_u64()?).ok()?,
        Value::String(text) => text.split('-').next()?.trim().parse().ok()?,
        _ => return None,
    };
    (port > 0).then_some(port)
}

fn native_tcp(protocol: &str) -> bool {
    !matches!(protocol, "hysteria2" | "tuic")
}

fn imported_tcp(protocol: &str, summary: &Value) -> bool {
    !matches!(protocol, "hysteria" | "hysteria2" | "tuic")
        && !matches!(summary["network"].as_str(), Some("kcp" | "mkcp" | "quic"))
}

/// Warnings matching Xray-core's REALITY checks. They never block anything.
pub fn risk_flags(public_port: u16, names: &[String]) -> Vec<RiskFlag> {
    let mut flags = BTreeSet::new();
    if public_port != 443 {
        flags.insert(RiskFlag {
            code: "port_not_443",
            value: public_port.to_string(),
        });
    }
    for name in names {
        let lower = name.trim().trim_end_matches('.').to_ascii_lowercase();
        if lower.is_empty() {
            continue;
        }
        if [".ru", ".ir", ".cn"].iter().any(|tld| lower.ends_with(tld)) {
            flags.insert(RiskFlag {
                code: "name_risky_tld",
                value: lower.clone(),
            });
        }
        if ["apple", "icloud", "microsoft"]
            .iter()
            .any(|brand| lower.contains(brand))
        {
            flags.insert(RiskFlag {
                code: "name_risky_brand",
                value: lower.clone(),
            });
        }
    }
    flags.into_iter().collect()
}

/// Merges nodes that share a public endpoint into one measurement target.
pub fn group(candidates: Vec<Candidate>) -> Vec<Target> {
    let mut grouped: BTreeMap<(String, u16), Target> = BTreeMap::new();
    for candidate in candidates {
        let target = grouped
            .entry((candidate.host.clone(), candidate.port))
            .or_insert_with(|| Target {
                host: candidate.host.clone(),
                port: candidate.port,
                nodes: Vec::new(),
                tcp: false,
                risks: Vec::new(),
            });
        target.tcp |= candidate.tcp;
        target.nodes.push(candidate.info);
        if let Some(names) = &candidate.reality_names {
            let mut merged: BTreeSet<_> = target.risks.drain(..).collect();
            merged.extend(risk_flags(candidate.port, names));
            target.risks = merged.into_iter().collect();
        }
    }
    grouped.into_values().collect()
}

#[derive(FromRow)]
struct NativeRow {
    id: i64,
    name: String,
    server_id: i64,
    server_name: String,
    protocol: String,
    public_host: String,
    port: i32,
    public_port: Option<String>,
    sni: String,
    handshake_server: Option<String>,
}

#[derive(FromRow)]
struct ImportedRow {
    id: i64,
    display: Value,
    server_id: i64,
    server_name: String,
}

/// A plaintext VLESS listener over HTTP sits behind a tunnel or CDN; clients
/// enter at the edge, so its own host and port are not an entrance to probe.
fn tunnel_origin(protocol: &str, tls: Option<i64>, network: Option<&str>) -> bool {
    protocol == "vless-tunnel"
        || (protocol == "vless" && tls == Some(0) && matches!(network, Some("ws" | "httpupgrade")))
}

fn native_candidate(row: NativeRow) -> Option<Candidate> {
    if tunnel_origin(&row.protocol, None, None) {
        return None;
    }
    let host = normalize_host(&row.public_host)?;
    let port = row
        .public_port
        .as_deref()
        .and_then(|port| port.parse::<u16>().ok())
        .or_else(|| u16::try_from(row.port).ok())
        .filter(|port| *port > 0)?;
    let reality_names = (row.protocol == "vless-reality").then(|| {
        std::iter::once(row.sni.clone())
            .chain(row.handshake_server.clone())
            .collect()
    });
    Some(Candidate {
        tcp: native_tcp(&row.protocol),
        reality_names,
        host,
        port,
        info: NodeInfo {
            kind: "native",
            id: row.id,
            name: row.name,
            server_id: row.server_id,
            server_name: row.server_name,
            protocol: row.protocol,
        },
    })
}

fn imported_candidate(row: ImportedRow) -> Option<Candidate> {
    let display = &row.display;
    let host = normalize_host(display["public_host"].as_str()?)?;
    let port = first_port(&display["public_port"])?;
    let protocol = display["protocol"].as_str()?.to_owned();
    let summary = &display["protocol_summary"];
    if tunnel_origin(
        &protocol,
        summary["tls"].as_i64(),
        summary["network"].as_str(),
    ) {
        return None;
    }
    // Only the plaintext display summary is read; the encrypted archive stays closed.
    let sni = summary["reality_sni"].as_str().map(str::to_owned);
    let reality = (protocol == "vless" && summary["tls"].as_i64() == Some(2)) || sni.is_some();
    Some(Candidate {
        tcp: imported_tcp(&protocol, summary),
        reality_names: reality.then(|| sni.into_iter().collect()),
        host,
        port,
        info: NodeInfo {
            kind: "nexus",
            id: row.id,
            name: display["name"].as_str().unwrap_or_default().to_owned(),
            server_id: row.server_id,
            server_name: row.server_name,
            protocol,
        },
    })
}

const NATIVE_QUERY: &str = "SELECT n.id,n.name,n.server_id,s.name AS server_name,n.protocol,n.public_host,n.port,n.settings->>'public_port' AS public_port,n.sni,n.settings->'reality'->>'handshake_server' AS handshake_server FROM nodes n JOIN servers s ON s.id=n.server_id";
const IMPORTED_QUERY: &str = "SELECT n.id,n.display,n.server_id,s.name AS server_name FROM singbox_nexus_nodes n JOIN servers s ON s.id=n.server_id";

/// Every endpoint the monitor would measure, enabled or not, deduplicated.
pub async fn enumerate(pool: &PgPool) -> ApiResult<Vec<Target>> {
    let native: Vec<NativeRow> = sqlx::query_as(&format!(
        "{NATIVE_QUERY} WHERE n.deleted_at IS NULL AND s.deleted_at IS NULL AND n.enabled ORDER BY n.id"
    ))
    .fetch_all(pool)
    .await?;
    let imported: Vec<ImportedRow> = sqlx::query_as(&format!(
        "{IMPORTED_QUERY} WHERE s.deleted_at IS NULL AND n.display->>'enabled'='true' AND COALESCE(n.display->>'machine_active','true')<>'false' ORDER BY n.id"
    ))
    .fetch_all(pool)
    .await?;
    Ok(group(
        native
            .into_iter()
            .filter_map(native_candidate)
            .chain(imported.into_iter().filter_map(imported_candidate))
            .collect(),
    ))
}

/// The endpoint of one configured node, whether or not it is currently enabled.
pub async fn native_endpoint(pool: &PgPool, id: i64) -> ApiResult<Option<(String, u16)>> {
    let row: Option<NativeRow> = sqlx::query_as(&format!(
        "{NATIVE_QUERY} WHERE n.id=$1 AND n.deleted_at IS NULL"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row
        .and_then(native_candidate)
        .map(|candidate| (candidate.host, candidate.port)))
}

pub async fn imported_endpoint(pool: &PgPool, id: i64) -> ApiResult<Option<(String, u16)>> {
    let row: Option<ImportedRow> = sqlx::query_as(&format!("{IMPORTED_QUERY} WHERE n.id=$1"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row
        .and_then(imported_candidate)
        .map(|candidate| (candidate.host, candidate.port)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn flags(port: u16, names: &[&str]) -> Vec<&'static str> {
        risk_flags(
            port,
            &names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>(),
        )
        .into_iter()
        .map(|flag| flag.code)
        .collect()
    }

    fn native(protocol: &str) -> NativeRow {
        NativeRow {
            id: 1,
            name: "原生".into(),
            server_id: 2,
            server_name: "服务器".into(),
            protocol: protocol.into(),
            public_host: "Node.Example.COM.".into(),
            port: 8443,
            public_port: None,
            sni: "www.example.com".into(),
            handshake_server: None,
        }
    }
    fn imported(display: Value) -> ImportedRow {
        ImportedRow {
            id: 5,
            display,
            server_id: 3,
            server_name: "主机".into(),
        }
    }

    #[test]
    fn tunnel_origins_are_not_probed() {
        assert!(native_candidate(native("vless-tunnel")).is_none());
        assert!(native_candidate(native("vless-reality")).is_some());
        let origin = |tls: i64, network: &str| {
            imported(
                json!({"public_host":"origin.example.com","public_port":"21443","protocol":"vless",
                "protocol_summary":{"tls":tls,"network":network}}),
            )
        };
        assert!(imported_candidate(origin(0, "ws")).is_none());
        assert!(imported_candidate(origin(0, "httpupgrade")).is_none());
        // TLS at the listener or a raw TCP listener is still a public entrance.
        assert!(imported_candidate(origin(1, "ws")).is_some());
        assert!(imported_candidate(origin(0, "tcp")).is_some());
    }

    #[test]
    fn hosts_are_canonicalised_for_deduplication() {
        assert_eq!(
            normalize_host(" Node.Example.COM. ").as_deref(),
            Some("node.example.com")
        );
        assert_eq!(
            normalize_host("[2001:DB8::0:1]").as_deref(),
            Some("2001:db8::1")
        );
        assert_eq!(normalize_host("192.0.2.1").as_deref(), Some("192.0.2.1"));
        assert_eq!(normalize_host(""), None);
        assert_eq!(normalize_host("bad host"), None);
        assert_eq!(normalize_host(&"a".repeat(254)), None);
    }

    #[test]
    fn imported_port_ranges_use_their_first_port() {
        assert_eq!(first_port(&json!("443")), Some(443));
        assert_eq!(first_port(&json!("21000-21020")), Some(21000));
        assert_eq!(first_port(&json!(8443)), Some(8443));
        for invalid in [json!(0), json!("0"), json!("a"), json!(70000), json!(null)] {
            assert_eq!(first_port(&invalid), None);
        }
    }

    #[test]
    fn the_standard_port_and_a_neutral_name_raise_no_flag() {
        assert!(flags(443, &["www.example.com"]).is_empty());
    }

    #[test]
    fn a_public_port_other_than_443_is_flagged() {
        assert_eq!(flags(8443, &[]), vec!["port_not_443"]);
    }

    #[test]
    fn risky_names_are_flagged_by_suffix_and_substring() {
        for name in ["cdn.example.ru", "EXAMPLE.IR", "www.example.cn."] {
            assert_eq!(flags(443, &[name]), vec!["name_risky_tld"], "{name}");
        }
        for name in [
            "www.apple.com",
            "swcdn.APPLE.example",
            "mask.icloud.com",
            "login.microsoftonline.example",
        ] {
            assert_eq!(flags(443, &[name]), vec!["name_risky_brand"], "{name}");
        }
        assert!(flags(443, &["example.run", "ru.example.com", "cn.example.org"]).is_empty());
        assert_eq!(
            flags(8443, &["apple.cn", "apple.cn"]),
            vec!["name_risky_brand", "name_risky_tld", "port_not_443"]
        );
    }

    #[test]
    fn only_reality_nodes_collect_risk_names() {
        let reality = native_candidate(native("vless-reality")).unwrap();
        assert_eq!(reality.host, "node.example.com");
        assert_eq!(reality.reality_names, Some(vec!["www.example.com".into()]));
        let other = native_candidate(native("anytls")).unwrap();
        assert_eq!(other.reality_names, None);
    }

    #[test]
    fn native_public_port_overrides_the_listen_port() {
        let mut row = native("vless-reality");
        row.public_port = Some("443".into());
        row.handshake_server = Some("mask.example.ru".into());
        let candidate = native_candidate(row).unwrap();
        assert_eq!(candidate.port, 443);
        assert_eq!(
            candidate.reality_names,
            Some(vec!["www.example.com".into(), "mask.example.ru".into()])
        );
    }

    #[test]
    fn udp_protocols_are_not_tcp_targets() {
        assert!(!native_candidate(native("hysteria2")).unwrap().tcp);
        assert!(!native_candidate(native("tuic")).unwrap().tcp);
        assert!(native_candidate(native("snell-v6")).unwrap().tcp);
        for (protocol, summary, tcp) in [
            ("hysteria", json!({"version":2}), false),
            ("tuic", json!({}), false),
            ("vless", json!({"network":"kcp"}), false),
            ("vmess", json!({"network":"ws"}), true),
            ("shadowsocks", json!({}), true),
        ] {
            let row = imported(json!({"name":"导入","public_host":"203.0.113.1",
                "public_port":"443","protocol":protocol,"protocol_summary":summary}));
            assert_eq!(imported_candidate(row).unwrap().tcp, tcp, "{protocol}");
        }
    }

    #[test]
    fn imported_flags_use_the_plaintext_summary_and_omit_what_it_lacks() {
        let with_name = imported(json!({"name":"导入","public_host":"203.0.113.1",
            "public_port":"443","protocol":"vless",
            "protocol_summary":{"tls":2,"reality_sni":"www.icloud.com"}}));
        let candidate = imported_candidate(with_name).unwrap();
        assert_eq!(
            candidate.reality_names,
            Some(vec!["www.icloud.com".to_owned()])
        );
        // REALITY known from the summary, name only in the encrypted archive.
        let without_name = imported(json!({"name":"导入","public_host":"203.0.113.1",
            "public_port":"8443","protocol":"vless","protocol_summary":{"tls":2}}));
        let targets = group(vec![imported_candidate(without_name).unwrap()]);
        assert_eq!(
            targets[0].risks,
            vec![RiskFlag {
                code: "port_not_443",
                value: "8443".into()
            }]
        );
        let not_reality = imported(json!({"name":"导入","public_host":"203.0.113.1",
            "public_port":"8443","protocol":"vless","protocol_summary":{"tls":1}}));
        let targets = group(vec![imported_candidate(not_reality).unwrap()]);
        assert!(targets[0].risks.is_empty());
    }

    #[test]
    fn nodes_on_one_endpoint_merge_and_any_tcp_node_keeps_it_measurable() {
        let mut first = native_candidate(native("vless-reality")).unwrap();
        first.host = "203.0.113.1".into();
        first.port = 443;
        first.reality_names = Some(vec!["www.apple.com".into()]);
        let mut second = first.clone();
        second.tcp = false;
        second.info.id = 2;
        second.info.name = "第二个".into();
        second.reality_names = None;
        let mut elsewhere = first.clone();
        elsewhere.port = 8443;
        let targets = group(vec![second, first, elsewhere]);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].port, 443);
        assert_eq!(targets[0].nodes.len(), 2);
        assert!(targets[0].tcp);
        assert_eq!(targets[0].risks[0].code, "name_risky_brand");
        assert_eq!(targets[0].node_names(), "第二个、原生");
        assert_eq!(targets[0].server_names(), "服务器");
    }

    #[test]
    fn an_endpoint_where_every_node_is_udp_is_not_measurable() {
        let udp = native_candidate(native("hysteria2")).unwrap();
        assert!(!group(vec![udp])[0].tcp);
    }

    #[test]
    fn imported_rows_missing_an_endpoint_are_skipped() {
        for display in [
            json!({"public_port":"443","protocol":"vless"}),
            json!({"public_host":"203.0.113.1","protocol":"vless"}),
            json!({"public_host":"203.0.113.1","public_port":"443"}),
        ] {
            assert!(imported_candidate(imported(display)).is_none());
        }
    }
}

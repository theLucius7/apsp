//! Clash Meta (mihomo) client profiles.
//!
//! The profile is derived from the same client outbounds as the sing-box
//! subscription, so both formats always describe the same authorized nodes.
//! An outbound whose meaning Clash cannot express exactly is left out and
//! listed in the profile header instead of being approximated.
use crate::{Node, authorized_nodes, client::ClientError, client::ExternalClientNode, protocols};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

pub const SELECT_GROUP: &str = "节点选择";
pub const AUTO_GROUP: &str = "自动选择";
const TEST_URL: &str = "https://www.gstatic.com/generate_204";

/// A rendered profile and the nodes that could not be expressed in it.
#[derive(Debug)]
pub struct ClashProfile {
    pub content: String,
    pub included: usize,
    pub skipped: Vec<(String, &'static str)>,
}

type Converted = Result<Value, &'static str>;

fn unsupported_protocol() -> &'static str {
    "Clash 不支持该协议"
}

fn object(value: &Value) -> Result<&Map<String, Value>, &'static str> {
    value.as_object().ok_or("出站定义无效")
}

/// Rejects any field the conversion does not understand, so no option is
/// silently dropped.
fn only(value: &Map<String, Value>, keys: &[&str]) -> Result<(), &'static str> {
    if value.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err("包含 Clash 无法等价表达的参数");
    }
    Ok(())
}

fn text<'a>(value: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn flag(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool) == Some(true)
}

/// Durations as sing-box writes them ("30s", "500ms", "1m") in the given unit.
fn duration(value: &Value, millis: bool) -> Result<u64, &'static str> {
    let invalid = "时长参数无效";
    let text = value.as_str().ok_or(invalid)?;
    let (number, scale) = if let Some(number) = text.strip_suffix("ms") {
        (number, if millis { 1 } else { 0 })
    } else if let Some(number) = text.strip_suffix('s') {
        (number, if millis { 1000 } else { 1 })
    } else if let Some(number) = text.strip_suffix('m') {
        (number, if millis { 60_000 } else { 60 })
    } else if let Some(number) = text.strip_suffix('h') {
        (number, if millis { 3_600_000 } else { 3600 })
    } else {
        return Err(invalid);
    };
    let number: u64 = number.parse().map_err(|_| invalid)?;
    if scale == 0 {
        // Sub-second values cannot be written in a seconds field.
        return if number.is_multiple_of(1000) {
            Ok(number / 1000)
        } else {
            Err(invalid)
        };
    }
    number.checked_mul(scale).ok_or(invalid)
}

fn network_udp(outbound: &Map<String, Value>) -> Result<bool, &'static str> {
    let Some(network) = outbound.get("network") else {
        return Ok(true);
    };
    let values: Vec<&str> = match network {
        Value::String(value) => value.split(',').collect(),
        Value::Array(values) => values.iter().filter_map(Value::as_str).collect(),
        _ => return Err("网络参数无效"),
    };
    if !values.contains(&"tcp") {
        return Err("Clash 不能表达仅 UDP 的节点");
    }
    Ok(values.contains(&"udp"))
}

/// TLS fields for the target type. `servername` is the VLESS/VMess spelling;
/// the other protocols use `sni`.
fn tls(
    outbound: &Map<String, Value>,
    kind: &str,
    proxy: &mut Map<String, Value>,
) -> Result<bool, &'static str> {
    let Some(tls) = outbound.get("tls") else {
        return Ok(false);
    };
    let tls = object(tls)?;
    only(
        tls,
        &[
            "enabled",
            "disable_sni",
            "server_name",
            "insecure",
            "alpn",
            "min_version",
            "max_version",
            "certificate",
            "utls",
            "reality",
        ],
    )?;
    if !flag(tls, "enabled") {
        return Ok(false);
    }
    if flag(tls, "disable_sni") {
        return Err("Clash 不能关闭 SNI");
    }
    let name_key = if matches!(kind, "vless" | "vmess") {
        "servername"
    } else {
        "sni"
    };
    if let Some(name) = text(tls, "server_name").filter(|name| !name.is_empty()) {
        proxy.insert(name_key.into(), json!(name));
    }
    if flag(tls, "insecure") {
        proxy.insert("skip-cert-verify".into(), json!(true));
    }
    if let Some(alpn) = tls.get("alpn") {
        proxy.insert("alpn".into(), alpn.clone());
    }
    if let Some(certificate) = tls.get("certificate") {
        // Only the QUIC protocols accept an inline CA in Clash Meta.
        if !matches!(kind, "hysteria2" | "tuic") {
            return Err("Clash 不能为该协议内联自签证书");
        }
        let pem = match certificate {
            Value::String(pem) => pem.clone(),
            Value::Array(lines) => lines
                .iter()
                .map(|line| line.as_str().ok_or("证书无效"))
                .collect::<Result<Vec<_>, _>>()?
                .join("\n"),
            _ => return Err("证书无效"),
        };
        proxy.insert("ca-str".into(), json!(pem));
    }
    if let Some(utls) = tls.get("utls") {
        let utls = object(utls)?;
        only(utls, &["enabled", "fingerprint"])?;
        if flag(utls, "enabled") {
            if !matches!(kind, "vless" | "vmess" | "trojan" | "anytls") {
                return Err("Clash 不支持该协议的客户端指纹");
            }
            proxy.insert(
                "client-fingerprint".into(),
                json!(text(utls, "fingerprint").unwrap_or("chrome")),
            );
        }
    }
    if let Some(reality) = tls.get("reality") {
        let reality = object(reality)?;
        only(reality, &["enabled", "public_key", "short_id"])?;
        if flag(reality, "enabled") {
            if kind != "vless" {
                return Err("Reality 只支持 VLESS");
            }
            proxy.insert(
                "reality-opts".into(),
                json!({"public-key": reality.get("public_key").ok_or("缺少 Reality 公钥")?,
                       "short-id": text(reality, "short_id").unwrap_or("")}),
            );
            // Clash Meta needs an explicit fingerprint for Reality.
            proxy
                .entry("client-fingerprint")
                .or_insert_with(|| json!("chrome"));
        }
    }
    Ok(true)
}

/// Clash represents HTTP request headers as single strings. Multiple values
/// cannot be joined without changing their meaning for arbitrary headers.
fn header_value(value: &Value) -> Converted {
    match value {
        Value::String(_) => Ok(value.clone()),
        Value::Array(values) if values.len() == 1 && values[0].is_string() => Ok(values[0].clone()),
        _ => Err("Clash 不能等价表达多值或空 HTTP 请求头"),
    }
}

fn headers(value: &Value) -> Result<Map<String, Value>, &'static str> {
    object(value)?
        .iter()
        .map(|(name, value)| Ok((name.clone(), header_value(value)?)))
        .collect()
}

fn transport(
    outbound: &Map<String, Value>,
    kind: &str,
    proxy: &mut Map<String, Value>,
) -> Result<(), &'static str> {
    let Some(transport) = outbound.get("transport") else {
        return Ok(());
    };
    let transport = object(transport)?;
    match text(transport, "type") {
        Some("ws") => {
            only(
                transport,
                &[
                    "type",
                    "path",
                    "headers",
                    "max_early_data",
                    "early_data_header_name",
                ],
            )?;
            let mut options = Map::new();
            if let Some(path) = transport.get("path") {
                options.insert("path".into(), path.clone());
            }
            if let Some(value) = transport.get("headers") {
                options.insert("headers".into(), Value::Object(headers(value)?));
            }
            if let Some(early) = transport.get("max_early_data") {
                options.insert("max-early-data".into(), early.clone());
            }
            if let Some(name) = transport.get("early_data_header_name") {
                options.insert("early-data-header-name".into(), name.clone());
            }
            proxy.insert("network".into(), json!("ws"));
            proxy.insert("ws-opts".into(), Value::Object(options));
        }
        Some("httpupgrade") => {
            only(transport, &["type", "path", "host", "headers"])?;
            let mut headers = transport
                .get("headers")
                .map(headers)
                .transpose()?
                .unwrap_or_default();
            if let Some(host) = transport.get("host") {
                headers.insert("Host".into(), header_value(host)?);
            }
            let mut options = json!({"v2ray-http-upgrade": true});
            if let Some(path) = transport.get("path") {
                options["path"] = path.clone();
            }
            if !headers.is_empty() {
                options["headers"] = Value::Object(headers);
            }
            proxy.insert("network".into(), json!("ws"));
            proxy.insert("ws-opts".into(), options);
        }
        Some("grpc") => {
            // Keepalive timings are client tuning; the service name is the identity.
            only(
                transport,
                &[
                    "type",
                    "service_name",
                    "idle_timeout",
                    "ping_timeout",
                    "permit_without_stream",
                ],
            )?;
            proxy.insert("network".into(), json!("grpc"));
            proxy.insert(
                "grpc-opts".into(),
                json!({"grpc-service-name": text(transport, "service_name").unwrap_or("")}),
            );
        }
        Some("http") => {
            if kind == "trojan" {
                return Err("Clash 的 Trojan 不支持 HTTP/2 传输");
            }
            if !outbound
                .get("tls")
                .and_then(Value::as_object)
                .is_some_and(|tls| flag(tls, "enabled"))
            {
                return Err("Clash 不能将未启用 TLS 的 HTTP 传输等价转换为 HTTP/2");
            }
            only(
                transport,
                &[
                    "type",
                    "host",
                    "path",
                    "method",
                    "headers",
                    "idle_timeout",
                    "ping_timeout",
                ],
            )?;
            // sing-box defaults to PUT; mihomo's H2 transport always uses PUT.
            if transport
                .get("method")
                .is_some_and(|method| !matches!(method.as_str(), Some("" | "PUT")))
            {
                return Err("Clash 的 HTTP/2 传输只支持 PUT 请求方法");
            }
            if let Some(headers) = transport.get("headers")
                && !object(headers)?.is_empty()
            {
                return Err("Clash 的 HTTP/2 传输不能保留自定义请求头");
            }
            let mut options = Map::new();
            if let Some(host) = transport.get("host") {
                let host = match host {
                    Value::String(host) => json!([host]),
                    other => other.clone(),
                };
                options.insert("host".into(), host);
            }
            if let Some(path) = transport.get("path") {
                options.insert("path".into(), path.clone());
            }
            proxy.insert("network".into(), json!("h2"));
            proxy.insert("h2-opts".into(), Value::Object(options));
        }
        _ => return Err("Clash 不支持该传输方式"),
    }
    Ok(())
}

fn multiplex(
    outbound: &Map<String, Value>,
    proxy: &mut Map<String, Value>,
) -> Result<(), &'static str> {
    let Some(mux) = outbound.get("multiplex") else {
        return Ok(());
    };
    let mux = object(mux)?;
    if !flag(mux, "enabled") {
        return Ok(());
    }
    let mut smux = json!({"enabled": true});
    for (from, to) in [
        ("protocol", "protocol"),
        ("max_connections", "max-connections"),
        ("min_streams", "min-streams"),
        ("max_streams", "max-streams"),
        ("padding", "padding"),
    ] {
        if let Some(value) = mux.get(from) {
            smux[to] = value.clone();
        }
    }
    if let Some(brutal) = mux.get("brutal").and_then(Value::as_object)
        && flag(brutal, "enabled")
    {
        smux["brutal-opts"] = json!({"enabled": true,
            "up": brutal.get("up_mbps").cloned().unwrap_or(Value::Null),
            "down": brutal.get("down_mbps").cloned().unwrap_or(Value::Null)});
    }
    proxy.insert("smux".into(), smux);
    Ok(())
}

fn udp_over_tcp(outbound: &Map<String, Value>, proxy: &mut Map<String, Value>) {
    match outbound.get("udp_over_tcp") {
        Some(Value::Bool(true)) => {
            proxy.insert("udp-over-tcp".into(), json!(true));
        }
        Some(Value::Object(uot)) if uot.get("enabled") == Some(&json!(true)) => {
            proxy.insert("udp-over-tcp".into(), json!(true));
            proxy.insert(
                "udp-over-tcp-version".into(),
                uot.get("version").cloned().unwrap_or(json!(2)),
            );
        }
        _ => {}
    }
}

/// Converts one sing-box client outbound into a Clash Meta proxy.
pub fn proxy(outbound: &Value, name: &str) -> Converted {
    let outbound = object(outbound)?;
    if outbound.contains_key("detour") {
        return Err("Clash 不能表达前置跳板");
    }
    let kind = text(outbound, "type").ok_or("出站定义无效")?;
    let common = [
        "type",
        "tag",
        "server",
        "server_port",
        "network",
        "tcp_fast_open",
        "tcp_multi_path",
        "domain_resolver",
    ];
    let extra: &[&str] = match kind {
        "vless" => &[
            "uuid",
            "flow",
            "tls",
            "packet_encoding",
            "multiplex",
            "transport",
        ],
        "vmess" => &[
            "uuid",
            "security",
            "alter_id",
            "global_padding",
            "authenticated_length",
            "tls",
            "packet_encoding",
            "multiplex",
            "transport",
        ],
        "trojan" => &["password", "tls", "multiplex", "transport"],
        "shadowsocks" => &["method", "password", "udp_over_tcp", "multiplex"],
        // Bandwidth profile tuning has no Clash counterpart and does not change identity.
        "hysteria2" => &[
            "password",
            "up_mbps",
            "down_mbps",
            "obfs",
            "tls",
            "server_ports",
            "hop_interval",
            "bbr_profile",
        ],
        "tuic" => &[
            "uuid",
            "password",
            "congestion_control",
            "udp_relay_mode",
            "udp_over_stream",
            "zero_rtt_handshake",
            "heartbeat",
            "tls",
        ],
        "anytls" => &[
            "password",
            "idle_session_check_interval",
            "idle_session_timeout",
            "min_idle_session",
            "tls",
        ],
        "socks" => &["version", "username", "password"],
        "http" => &["username", "password", "path", "headers", "tls"],
        _ => return Err(unsupported_protocol()),
    };
    let allowed: Vec<&str> = common.iter().chain(extra).copied().collect();
    only(outbound, &allowed)?;
    let server = text(outbound, "server").ok_or("缺少服务器地址")?;
    let port = outbound
        .get("server_port")
        .and_then(Value::as_u64)
        .filter(|port| (1..=65535).contains(port))
        .ok_or("端口无效")?;
    let target = match kind {
        "shadowsocks" => "ss",
        "socks" => "socks5",
        other => other,
    };
    let mut proxy = Map::new();
    proxy.insert("name".into(), json!(name));
    proxy.insert("type".into(), json!(target));
    proxy.insert("server".into(), json!(server));
    proxy.insert("port".into(), json!(port));
    let udp = network_udp(outbound)?;
    if kind != "http" {
        proxy.insert("udp".into(), json!(udp));
    }
    if flag(outbound, "tcp_fast_open") {
        proxy.insert("tfo".into(), json!(true));
    }
    if flag(outbound, "tcp_multi_path") {
        proxy.insert("mptcp".into(), json!(true));
    }
    let secured = tls(outbound, kind, &mut proxy)?;
    match kind {
        "vless" | "vmess" => {
            proxy.insert(
                "uuid".into(),
                outbound.get("uuid").ok_or("缺少 UUID")?.clone(),
            );
            if secured {
                proxy.insert("tls".into(), json!(true));
            }
            if kind == "vless" {
                match text(outbound, "flow").unwrap_or("") {
                    "" => {}
                    "xtls-rprx-vision" => {
                        proxy.insert("flow".into(), json!("xtls-rprx-vision"));
                    }
                    _ => return Err("Clash 不支持该流控"),
                }
            } else {
                proxy.insert(
                    "cipher".into(),
                    json!(text(outbound, "security").unwrap_or("auto")),
                );
                proxy.insert(
                    "alterId".into(),
                    outbound.get("alter_id").cloned().unwrap_or(json!(0)),
                );
                if flag(outbound, "global_padding") {
                    proxy.insert("global-padding".into(), json!(true));
                }
                if flag(outbound, "authenticated_length") {
                    proxy.insert("authenticated-length".into(), json!(true));
                }
            }
            if let Some(encoding) = text(outbound, "packet_encoding").filter(|e| !e.is_empty()) {
                proxy.insert("packet-encoding".into(), json!(encoding));
            }
            transport(outbound, kind, &mut proxy)?;
            multiplex(outbound, &mut proxy)?;
        }
        "trojan" => {
            if !secured {
                return Err("Clash 的 Trojan 必须启用 TLS");
            }
            proxy.insert(
                "password".into(),
                outbound.get("password").ok_or("缺少密码")?.clone(),
            );
            transport(outbound, kind, &mut proxy)?;
            multiplex(outbound, &mut proxy)?;
        }
        "shadowsocks" => {
            proxy.insert(
                "cipher".into(),
                outbound.get("method").ok_or("缺少加密方式")?.clone(),
            );
            proxy.insert(
                "password".into(),
                outbound.get("password").ok_or("缺少密码")?.clone(),
            );
            udp_over_tcp(outbound, &mut proxy);
            multiplex(outbound, &mut proxy)?;
        }
        "hysteria2" => {
            if !secured {
                return Err("Hysteria2 必须启用 TLS");
            }
            proxy.insert(
                "password".into(),
                outbound.get("password").ok_or("缺少密码")?.clone(),
            );
            for (from, to) in [("up_mbps", "up"), ("down_mbps", "down")] {
                if let Some(mbps) = outbound.get(from).and_then(Value::as_u64) {
                    proxy.insert(to.into(), json!(format!("{mbps} Mbps")));
                }
            }
            if let Some(obfs) = outbound.get("obfs") {
                let obfs = object(obfs)?;
                if text(obfs, "type") != Some("salamander") {
                    return Err("Clash 不支持该混淆");
                }
                proxy.insert("obfs".into(), json!("salamander"));
                proxy.insert(
                    "obfs-password".into(),
                    obfs.get("password").ok_or("缺少混淆密码")?.clone(),
                );
            }
            if let Some(ports) = outbound.get("server_ports").and_then(Value::as_array)
                && !ports.is_empty()
            {
                let ranges = ports
                    .iter()
                    .map(|range| range.as_str().map(|range| range.replace(':', "-")))
                    .collect::<Option<Vec<_>>>()
                    .ok_or("端口跳跃范围无效")?;
                proxy.insert("ports".into(), json!(ranges.join(",")));
            }
            if let Some(interval) = outbound.get("hop_interval") {
                proxy.insert("hop-interval".into(), json!(duration(interval, false)?));
            }
        }
        "tuic" => {
            if !secured {
                return Err("TUIC 必须启用 TLS");
            }
            proxy.insert(
                "uuid".into(),
                outbound.get("uuid").ok_or("缺少 UUID")?.clone(),
            );
            proxy.insert(
                "password".into(),
                outbound.get("password").ok_or("缺少密码")?.clone(),
            );
            if let Some(control) = text(outbound, "congestion_control") {
                proxy.insert("congestion-controller".into(), json!(control));
            }
            match text(outbound, "udp_relay_mode") {
                None | Some("") => {}
                Some(mode @ ("native" | "quic")) => {
                    proxy.insert("udp-relay-mode".into(), json!(mode));
                }
                Some(_) => return Err("TUIC 的 UDP 转发模式无效"),
            }
            if flag(outbound, "udp_over_stream") {
                proxy.insert("udp-over-stream".into(), json!(true));
            }
            if flag(outbound, "zero_rtt_handshake") {
                proxy.insert("reduce-rtt".into(), json!(true));
            }
            if let Some(heartbeat) = outbound.get("heartbeat") {
                proxy.insert(
                    "heartbeat-interval".into(),
                    json!(duration(heartbeat, true)?),
                );
            }
        }
        "anytls" => {
            if !secured {
                return Err("AnyTLS 必须启用 TLS");
            }
            proxy.insert(
                "password".into(),
                outbound.get("password").ok_or("缺少密码")?.clone(),
            );
            for (from, to) in [
                ("idle_session_check_interval", "idle-session-check-interval"),
                ("idle_session_timeout", "idle-session-timeout"),
            ] {
                if let Some(value) = outbound.get(from) {
                    proxy.insert(to.into(), json!(duration(value, false)?));
                }
            }
            if let Some(count) = outbound.get("min_idle_session") {
                proxy.insert("min-idle-session".into(), count.clone());
            }
        }
        "socks" => {
            if text(outbound, "version").is_some_and(|version| version != "5") {
                return Err("Clash 只支持 SOCKS5");
            }
            for key in ["username", "password"] {
                if let Some(value) = outbound.get(key) {
                    proxy.insert(key.into(), value.clone());
                }
            }
        }
        "http" => {
            // Normalized HTTP outbounds retain an empty path for ordinary CONNECT.
            if outbound
                .get("path")
                .is_some_and(|path| path.as_str() != Some(""))
            {
                return Err("Clash 的 HTTP 代理不能保留自定义请求路径");
            }
            if secured {
                proxy.insert("tls".into(), json!(true));
            }
            for key in ["username", "password"] {
                if let Some(value) = outbound.get(key) {
                    proxy.insert(key.into(), value.clone());
                }
            }
            if let Some(value) = outbound.get("headers") {
                proxy.insert("headers".into(), Value::Object(headers(value)?));
            }
        }
        _ => return Err(unsupported_protocol()),
    }
    Ok(Value::Object(proxy))
}

/// Unique display names; group names and built-in policies stay reserved.
fn unique(name: &str, used: &mut BTreeSet<String>) -> String {
    let base = name.trim();
    let base = if base.is_empty() { "节点" } else { base };
    let mut candidate = base.to_string();
    let mut counter = 2;
    while used.contains(&candidate)
        || [
            SELECT_GROUP,
            AUTO_GROUP,
            "DIRECT",
            "REJECT",
            "REJECT-DROP",
            "COMPATIBLE",
            "PASS",
            "PASS-RULE",
            "GLOBAL",
        ]
        .contains(&candidate.as_str())
    {
        candidate = format!("{base} ({counter})");
        counter += 1;
    }
    used.insert(candidate.clone());
    candidate
}

/// Builds the user's Clash Meta profile from the same nodes as the sing-box
/// subscription: one manual selection group, one latency test group and a
/// final rule, mirroring the sing-box client's single selector.
pub fn compile_clash(
    nodes: &[Node],
    user_id: i64,
    external: &[ExternalClientNode],
) -> Result<ClashProfile, ClientError> {
    let mut sources: Vec<(String, Result<Value, &'static str>)> = Vec::new();
    for (node, access) in authorized_nodes(nodes, user_id)? {
        sources.push((node.name.clone(), Ok(protocols::client(node, access))));
    }
    let mut external: Vec<_> = external.iter().collect();
    external.sort_by_key(|node| (node.sort_order, node.id));
    for node in external {
        let rendered = node
            .outbound
            .render(&format!("external-node-{}", node.id), None, None)
            .map_err(|_| "外部节点定义无效");
        sources.push((node.name.clone(), rendered));
    }
    Ok(render(sources))
}

/// Assembles the profile from display names and their client outbounds.
fn render(sources: Vec<(String, Result<Value, &'static str>)>) -> ClashProfile {
    let mut used = BTreeSet::new();
    let mut proxies = Vec::new();
    let mut names = Vec::new();
    let mut skipped = Vec::new();
    for (name, outbound) in sources {
        let display = name.trim().to_string();
        match outbound.and_then(|outbound| proxy(&outbound, &display)) {
            Ok(mut converted) => {
                let unique = unique(&display, &mut used);
                converted["name"] = json!(unique);
                names.push(unique);
                proxies.push(converted);
            }
            Err(reason) => skipped.push((display, reason)),
        }
    }
    let mut select = vec![json!(AUTO_GROUP)];
    select.extend(names.iter().map(|name| json!(name)));
    select.push(json!("DIRECT"));
    let profile = json!({
        "mixed-port": 7890,
        "allow-lan": false,
        "mode": "rule",
        "log-level": "warning",
        "ipv6": true,
        "proxies": proxies,
        "proxy-groups": [
            {"name": SELECT_GROUP, "type": "select", "proxies": select},
            {"name": AUTO_GROUP, "type": "url-test", "proxies": names,
             "url": TEST_URL, "interval": 300, "tolerance": 50},
        ],
        "rules": [format!("MATCH,{SELECT_GROUP}")],
    });
    let mut content = String::from("# apsp 订阅（Clash Meta / mihomo 格式）\n");
    if !skipped.is_empty() {
        content.push_str(
            "# 以下节点无法以 Clash 格式等价表达，未包含（请使用 sing-box 格式获取）：\n",
        );
        for (name, reason) in &skipped {
            content.push_str(&format!("#   {}：{reason}\n", comment(name)));
        }
    }
    yaml(&profile, 0, &mut content);
    ClashProfile {
        content,
        included: proxies_len(&profile),
        skipped,
    }
}

fn proxies_len(profile: &Value) -> usize {
    profile["proxies"].as_array().map_or(0, Vec::len)
}

/// Comment text cannot break out of its line.
fn comment(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// A YAML double-quoted scalar. Every character YAML does not print is escaped.
fn quoted(text: &str) -> String {
    let mut result = String::with_capacity(text.len() + 2);
    result.push('"');
    for c in text.chars() {
        match c {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\t' => result.push_str("\\t"),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}') => {
                result.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => result.push(c),
        }
    }
    result.push('"');
    result
}

fn key(text: &str) -> String {
    if !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        text.into()
    } else {
        quoted(text)
    }
}

fn scalar(value: &Value) -> Option<String> {
    Some(match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => quoted(value),
        Value::Array(values) if values.is_empty() => "[]".into(),
        Value::Object(values) if values.is_empty() => "{}".into(),
        _ => return None,
    })
}

/// Keys written first, in this order; the rest follow alphabetically.
const LEADING: [&str; 12] = [
    "name",
    "type",
    "server",
    "port",
    "mixed-port",
    "allow-lan",
    "mode",
    "log-level",
    "ipv6",
    "proxies",
    "proxy-groups",
    "rules",
];

fn ordered(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by_key(|(name, _)| {
        LEADING
            .iter()
            .position(|leading| leading == name)
            .unwrap_or(LEADING.len())
    });
    entries
}

/// Block-style YAML for a JSON value.
fn yaml(value: &Value, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match value {
        Value::Object(map) => {
            for (name, child) in ordered(map) {
                match scalar(child) {
                    Some(text) => out.push_str(&format!("{pad}{}: {text}\n", key(name))),
                    None => {
                        out.push_str(&format!("{pad}{}:\n", key(name)));
                        yaml(child, indent + 2, out);
                    }
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                match (scalar(item), item) {
                    (Some(text), _) => out.push_str(&format!("{pad}- {text}\n")),
                    (None, Value::Object(_)) => {
                        let mut nested = String::new();
                        yaml(item, indent + 2, &mut nested);
                        // The first mapping line moves onto the dash.
                        let nested =
                            nested.replacen(&" ".repeat(indent + 2), &format!("{pad}- "), 1);
                        out.push_str(&nested);
                    }
                    (None, _) => {
                        out.push_str(&format!("{pad}-\n"));
                        yaml(item, indent + 2, out);
                    }
                }
            }
        }
        other => out.push_str(&format!("{pad}{}\n", scalar(other).unwrap_or_default())),
    }
}

#[cfg(test)]
mod regression_tests;

#[cfg(test)]
mod normalized_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reality_vision_maps_to_clash_meta_fields() {
        let outbound = json!({"type":"vless","tag":"node-1","server":"node.example.invalid","server_port":443,
            "uuid":"00000000-0000-4000-8000-000000000001","flow":"xtls-rprx-vision",
            "tls":{"enabled":true,"server_name":"www.example.com","utls":{"enabled":true,"fingerprint":"chrome"},
                   "reality":{"enabled":true,"public_key":"TEST_ONLY_KEY","short_id":"ab"}}});
        let proxy = proxy(&outbound, "香港").unwrap();
        assert_eq!(
            proxy,
            json!({"name":"香港","type":"vless","server":"node.example.invalid","port":443,"udp":true,
            "servername":"www.example.com","client-fingerprint":"chrome",
            "reality-opts":{"public-key":"TEST_ONLY_KEY","short-id":"ab"},
            "uuid":"00000000-0000-4000-8000-000000000001","tls":true,"flow":"xtls-rprx-vision"})
        );
    }

    #[test]
    fn quic_protocols_keep_obfuscation_ports_and_inline_ca() {
        let hysteria = json!({"type":"hysteria2","tag":"x","server":"h.example.invalid","server_port":8443,
            "password":"TEST_ONLY","up_mbps":20,"down_mbps":100,"obfs":{"type":"salamander","password":"TEST_ONLY_OBFS"},
            "server_ports":["20000:20100"],"hop_interval":"30s",
            "tls":{"enabled":true,"server_name":"h.example.invalid","alpn":["h3"],"certificate":["-----BEGIN CERTIFICATE-----","TEST","-----END CERTIFICATE-----"]}});
        let proxy = proxy(&hysteria, "hy2").unwrap();
        assert_eq!(proxy["obfs"], "salamander");
        assert_eq!(proxy["ports"], "20000-20100");
        assert_eq!(proxy["hop-interval"], 30);
        assert_eq!(proxy["up"], "20 Mbps");
        assert_eq!(
            proxy["ca-str"],
            "-----BEGIN CERTIFICATE-----\nTEST\n-----END CERTIFICATE-----"
        );
        let tuic = json!({"type":"tuic","tag":"t","server":"t.example.invalid","server_port":443,
            "uuid":"00000000-0000-4000-8000-000000000002","password":"TEST_ONLY","heartbeat":"10s",
            "udp_relay_mode":"quic","zero_rtt_handshake":true,"tls":{"enabled":true,"server_name":"t.example.invalid","alpn":["h3"]}});
        let tuic = super::proxy(&tuic, "tuic").unwrap();
        assert_eq!(tuic["heartbeat-interval"], 10000);
        assert_eq!(tuic["udp-relay-mode"], "quic");
        assert_eq!(tuic["reduce-rtt"], true);
    }

    #[test]
    fn transports_mux_and_shadowsocks_options_convert() {
        let vmess = json!({"type":"vmess","tag":"v","server":"v.example.invalid","server_port":443,
            "uuid":"00000000-0000-4000-8000-000000000003","security":"auto",
            "tls":{"enabled":true,"server_name":"v.example.invalid"},
            "transport":{"type":"httpupgrade","path":"/up","host":"v.example.invalid"},
            "multiplex":{"enabled":true,"protocol":"h2mux","max_streams":8,"padding":true}});
        let vmess = proxy(&vmess, "v").unwrap();
        assert_eq!(vmess["network"], "ws");
        assert_eq!(
            vmess["ws-opts"],
            json!({"v2ray-http-upgrade":true,"path":"/up","headers":{"Host":"v.example.invalid"}})
        );
        assert_eq!(
            vmess["smux"],
            json!({"enabled":true,"protocol":"h2mux","max-streams":8,"padding":true})
        );
        assert_eq!(vmess["cipher"], "auto");
        let ss = json!({"type":"shadowsocks","tag":"s","server":"s.example.invalid","server_port":8388,
            "method":"2022-blake3-aes-128-gcm","password":"TEST_ONLY:TEST_ONLY","network":"tcp",
            "udp_over_tcp":{"enabled":true,"version":2}});
        let ss = proxy(&ss, "s").unwrap();
        assert_eq!(ss["type"], "ss");
        assert_eq!(ss["udp"], false);
        assert_eq!(ss["udp-over-tcp-version"], 2);
    }

    #[test]
    fn inexpressible_outbounds_are_refused_not_approximated() {
        let snell = json!({"type":"snell","tag":"n","server":"n.example.invalid","server_port":1,"version":6,"psk":"x","userkey":"y"});
        assert!(proxy(&snell, "n").is_err());
        let detour = json!({"type":"trojan","tag":"t","server":"t.example.invalid","server_port":443,"password":"x",
            "tls":{"enabled":true},"detour":"other"});
        assert!(proxy(&detour, "t").is_err());
        let unknown = json!({"type":"vless","tag":"u","server":"u.example.invalid","server_port":443,
            "uuid":"00000000-0000-4000-8000-000000000004","tls":{"enabled":true,"ech":{"enabled":true}}});
        assert!(proxy(&unknown, "u").is_err());
        let udp_only = json!({"type":"shadowsocks","tag":"s","server":"s.example.invalid","server_port":1,
            "method":"aes-128-gcm","password":"x","network":"udp"});
        assert!(proxy(&udp_only, "s").is_err());
    }

    #[test]
    fn a_full_profile_lists_supported_nodes_and_reports_the_rest() {
        let tls = |name: &str| json!({"enabled":true,"server_name":name});
        let sources = vec![
            (
                "Reality".to_string(),
                Ok(
                    json!({"type":"vless","tag":"a","server":"r.example.invalid","server_port":443,
                "uuid":"00000000-0000-4000-8000-000000000001","flow":"xtls-rprx-vision",
                "tls":{"enabled":true,"server_name":"www.example.com","utls":{"enabled":true,"fingerprint":"chrome"},
                       "reality":{"enabled":true,"public_key":"jNXHt1yRo0vDuchQlIP6Z0ZvjT3KtzVI-T4E7RoLJS0","short_id":"0123abcd"}}}),
                ),
            ),
            (
                "VMess WS".to_string(),
                Ok(
                    json!({"type":"vmess","tag":"b","server":"v.example.invalid","server_port":443,
                "uuid":"00000000-0000-4000-8000-000000000002","security":"auto","tls":tls("v.example.invalid"),
                "transport":{"type":"ws","path":"/ws","headers":{"Host":["v.example.invalid"]}}}),
                ),
            ),
            (
                "VMess HTTPUpgrade".to_string(),
                Ok(
                    json!({"type":"vmess","tag":"b-upgrade","server":"v.example.invalid","server_port":443,
                "uuid":"00000000-0000-4000-8000-000000000002","security":"auto","tls":tls("v.example.invalid"),
                "transport":{"type":"httpupgrade","path":"/upgrade","host":"v.example.invalid",
                             "headers":{"X-Test":["TEST_ONLY"]}}}),
                ),
            ),
            (
                "Trojan gRPC".to_string(),
                Ok(
                    json!({"type":"trojan","tag":"c","server":"t.example.invalid","server_port":443,
                "password":"TEST_ONLY","tls":tls("t.example.invalid"),"transport":{"type":"grpc","service_name":"svc"}}),
                ),
            ),
            (
                "SS2022".to_string(),
                Ok(
                    json!({"type":"shadowsocks","tag":"d","server":"s.example.invalid","server_port":8388,
                "method":"2022-blake3-aes-128-gcm","password":"AAAAAAAAAAAAAAAAAAAAAA==:AAAAAAAAAAAAAAAAAAAAAA==",
                "udp_over_tcp":{"enabled":true,"version":2},"multiplex":{"enabled":true,"protocol":"smux","padding":true}}),
                ),
            ),
            (
                "Hysteria2".to_string(),
                Ok(
                    json!({"type":"hysteria2","tag":"e","server":"h.example.invalid","server_port":8443,
                "password":"TEST_ONLY","up_mbps":20,"down_mbps":100,"obfs":{"type":"salamander","password":"TEST_ONLY"},
                "server_ports":["20000:20100"],"hop_interval":"30s","tls":{"enabled":true,"server_name":"h.example.invalid","alpn":["h3"]}}),
                ),
            ),
            (
                "TUIC".to_string(),
                Ok(
                    json!({"type":"tuic","tag":"f","server":"q.example.invalid","server_port":443,
                "uuid":"00000000-0000-4000-8000-000000000003","password":"TEST_ONLY","congestion_control":"bbr",
                "heartbeat":"10s","tls":{"enabled":true,"server_name":"q.example.invalid","alpn":["h3"]}}),
                ),
            ),
            (
                "AnyTLS".to_string(),
                Ok(
                    json!({"type":"anytls","tag":"g","server":"y.example.invalid","server_port":443,
                "password":"TEST_ONLY","idle_session_timeout":"30s","tls":tls("y.example.invalid")}),
                ),
            ),
            (
                "SOCKS".to_string(),
                Ok(
                    json!({"type":"socks","tag":"h","server":"k.example.invalid","server_port":1080,"version":"5"}),
                ),
            ),
            (
                "PASS".to_string(),
                Ok(
                    json!({"type":"http","tag":"i","server":"p.example.invalid","server_port":8080,"username":"u","password":"p",
                "headers":{"X-Test":["TEST_ONLY"]}}),
                ),
            ),
            (
                "Snell".to_string(),
                Ok(
                    json!({"type":"snell","tag":"j","server":"n.example.invalid","server_port":1,"version":6}),
                ),
            ),
            (
                "Broken external\u{2028}external-controller: 127.0.0.1:19090\u{2029}#\u{feff}"
                    .to_string(),
                Err("外部节点定义无效"),
            ),
        ];
        let profile = render(sources);
        assert_eq!(profile.included, 10);
        assert_eq!(profile.skipped.len(), 2);
        assert!(
            profile
                .content
                .starts_with("# apsp 订阅（Clash Meta / mihomo 格式）\n# 以下节点")
        );
        assert!(profile.content.contains("#   Snell：Clash 不支持该协议\n"));
        assert!(profile.content.contains("rules:\n  - \"MATCH,节点选择\"\n"));
        assert!(profile.content.contains("  - name: \"PASS (2)\"\n"));
        assert!(profile.content.contains("Host: \"v.example.invalid\"\n"));
        assert!(profile.content.contains("X-Test: \"TEST_ONLY\"\n"));
        assert!(
            !profile
                .content
                .contains(['\u{2028}', '\u{2029}', '\u{feff}'])
        );
        assert!(!profile.content.contains("\nexternal-controller:"));
        // A real client can check this sample: `mihomo -t -f <path>`.
        if let Ok(path) = std::env::var("APSP_CLASH_SAMPLE") {
            std::fs::write(path, &profile.content).unwrap();
        }
    }

    #[test]
    fn yaml_output_quotes_every_string_and_keeps_names_unique() {
        let mut used = BTreeSet::new();
        assert_eq!(unique("节点选择", &mut used), "节点选择 (2)");
        assert_eq!(unique("a", &mut used), "a");
        assert_eq!(unique("a", &mut used), "a (2)");
        let mut out = String::new();
        yaml(
            &json!({"proxies":[{"name":"a\"b: #c\u{7f}","port":1,"alpn":["h3"]}],"rules":["MATCH,x"],"empty":[]}),
            0,
            &mut out,
        );
        assert_eq!(
            out,
            "proxies:\n  - name: \"a\\\"b: #c\\u007f\"\n    port: 1\n    alpn:\n      - \"h3\"\nrules:\n  - \"MATCH,x\"\nempty: []\n"
        );
    }
}

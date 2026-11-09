use super::{
    MAX_NODES, NodePreview, ParseError, ParseReason, ParseStatus, ParsedNode, display_name,
    outbound,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

fn string(value: &Map<String, Value>, key: &str) -> Result<String, ParseReason> {
    outbound::string(value, key)
}

fn number(value: &Value) -> Result<Value, ParseReason> {
    if let Some(number) = value.as_u64() {
        return Ok(number.into());
    }
    if let Some(number) = value.as_str().and_then(|value| value.parse::<u64>().ok()) {
        return Ok(number.into());
    }
    Err(outbound::invalid())
}

fn transfer(source: &Map<String, Value>, target: &mut Map<String, Value>, from: &str, to: &str) {
    if let Some(value) = source.get(from) {
        target.insert(to.into(), value.clone());
    }
}

fn checked_keys(value: &Map<String, Value>, keys: &[&str]) -> Result<(), ParseReason> {
    if value.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err(outbound::unsupported());
    }
    Ok(())
}

fn tls(source: &Map<String, Value>, kind: &str) -> Result<Option<Value>, ParseReason> {
    if source.contains_key("fingerprint") {
        return Err(ParseReason::new(
            "unsupported_parameter",
            "节点证书指纹的验证语义无法等价转换",
        ));
    }
    let mandatory = ["trojan", "hysteria2", "tuic", "anytls"].contains(&kind);
    let enabled = outbound::boolean(source, "tls", mandatory)?;
    let mut value = json!({"enabled":enabled})
        .as_object()
        .expect("literal object")
        .clone();
    if source.contains_key("servername") && source.contains_key("sni") {
        return Err(outbound::invalid());
    }
    transfer(source, &mut value, "servername", "server_name");
    transfer(source, &mut value, "sni", "server_name");
    transfer(source, &mut value, "skip-cert-verify", "insecure");
    transfer(source, &mut value, "alpn", "alpn");
    if let Some(fingerprint) = source.get("client-fingerprint") {
        if ["hysteria2", "tuic"].contains(&kind) {
            return Err(outbound::unsupported());
        }
        value.insert(
            "utls".into(),
            json!({"enabled":true,"fingerprint":fingerprint}),
        );
    }
    if let Some(reality) = source.get("reality-opts") {
        if kind != "vless" {
            return Err(outbound::invalid());
        }
        let reality = reality.as_object().ok_or_else(outbound::invalid)?;
        checked_keys(reality, &["public-key", "short-id"])?;
        let short_id = reality
            .get("short-id")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new()));
        value.insert("reality".into(), json!({"enabled":true,"public_key":reality.get("public-key").ok_or_else(outbound::credential)?,"short_id":short_id}));
    }
    if !enabled && value.len() > 1 {
        return Err(outbound::invalid());
    }
    Ok(enabled.then_some(Value::Object(value)))
}

fn transport(source: &Map<String, Value>) -> Result<Option<Value>, ParseReason> {
    let kind = source
        .get("network")
        .map(|value| value.as_str().ok_or_else(outbound::invalid))
        .unwrap_or(Ok("tcp"))?;
    let transport_keys = ["ws-opts", "grpc-opts", "h2-opts", "http-opts"];
    let selected = match kind {
        "ws" => Some("ws-opts"),
        "grpc" => Some("grpc-opts"),
        "h2" => Some("h2-opts"),
        "http" => Some("http-opts"),
        "tcp" => None,
        _ => {
            return Err(ParseReason::new(
                "unsupported_transport",
                "节点的传输方式尚未支持",
            ));
        }
    };
    if transport_keys
        .iter()
        .any(|key| source.contains_key(*key) && selected != Some(*key))
    {
        return Err(outbound::unsupported());
    }
    let Some(selected) = selected else {
        return Ok(None);
    };
    let empty = Map::new();
    let options = source
        .get(selected)
        .map(|value| value.as_object().ok_or_else(outbound::invalid))
        .transpose()?
        .unwrap_or(&empty);
    let mut transport = Map::new();
    match kind {
        "ws" => {
            checked_keys(
                options,
                &[
                    "path",
                    "headers",
                    "max-early-data",
                    "early-data-header-name",
                    "v2ray-http-upgrade",
                    "v2ray-http-upgrade-fast-open",
                ],
            )?;
            let upgrade = outbound::boolean(options, "v2ray-http-upgrade", false)?;
            if outbound::boolean(options, "v2ray-http-upgrade-fast-open", false)? {
                return Err(outbound::unsupported());
            }
            transport.insert(
                "type".into(),
                Value::String(if upgrade { "httpupgrade" } else { "ws" }.into()),
            );
            transfer(options, &mut transport, "path", "path");
            transfer(options, &mut transport, "headers", "headers");
            if upgrade {
                if options.contains_key("max-early-data")
                    || options.contains_key("early-data-header-name")
                {
                    return Err(outbound::unsupported());
                }
                if let Some(host) = options
                    .get("headers")
                    .and_then(Value::as_object)
                    .and_then(|headers| headers.get("Host").or_else(|| headers.get("host")))
                {
                    transport.insert("host".into(), host.clone());
                }
            } else {
                transfer(options, &mut transport, "max-early-data", "max_early_data");
                transfer(
                    options,
                    &mut transport,
                    "early-data-header-name",
                    "early_data_header_name",
                );
            }
        }
        "grpc" => {
            checked_keys(options, &["grpc-service-name"])?;
            transport.insert("type".into(), json!("grpc"));
            transfer(options, &mut transport, "grpc-service-name", "service_name");
        }
        "h2" => {
            checked_keys(options, &["host", "path"])?;
            transport.insert("type".into(), json!("http"));
            transfer(options, &mut transport, "host", "host");
            transfer(options, &mut transport, "path", "path");
        }
        "http" => {
            // HTTP header camouflage in VMess TCP is not sing-box's HTTP/2 transport.
            return Err(ParseReason::new(
                "unsupported_transport",
                "节点的 HTTP 伪装传输无法等价转换",
            ));
        }
        _ => return Err(outbound::unsupported()),
    }
    Ok(Some(Value::Object(transport)))
}

fn plugin(source: &Map<String, Value>, target: &mut Map<String, Value>) -> Result<(), ParseReason> {
    let Some(plugin) = source.get("plugin") else {
        if source.contains_key("plugin-opts") {
            return Err(outbound::invalid());
        }
        return Ok(());
    };
    let plugin = plugin.as_str().ok_or_else(outbound::invalid)?;
    let options = source
        .get("plugin-opts")
        .map(|value| value.as_object().ok_or_else(outbound::invalid))
        .transpose()?;
    let mut fields = Vec::new();
    match plugin {
        "obfs" => {
            target.insert("plugin".into(), json!("obfs-local"));
            if let Some(options) = options {
                checked_keys(options, &["mode", "host"])?;
                if let Some(mode) = options.get("mode") {
                    fields.push(format!(
                        "obfs={}",
                        escaped(mode.as_str().ok_or_else(outbound::invalid)?)
                    ));
                }
                if let Some(host) = options.get("host") {
                    fields.push(format!(
                        "obfs-host={}",
                        escaped(host.as_str().ok_or_else(outbound::invalid)?)
                    ));
                }
            }
        }
        "v2ray-plugin" => {
            target.insert("plugin".into(), json!("v2ray-plugin"));
            if let Some(options) = options {
                checked_keys(options, &["mode", "tls", "host", "path", "mux"])?;
                for key in ["mode", "host", "path"] {
                    if let Some(value) = options.get(key) {
                        fields.push(format!(
                            "{key}={}",
                            escaped(value.as_str().ok_or_else(outbound::invalid)?)
                        ));
                    }
                }
                if outbound::boolean(options, "tls", false)? {
                    fields.push("tls".into());
                }
                if let Some(mux) = options.get("mux") {
                    if let Some(enabled) = mux.as_bool() {
                        fields.push(format!("mux={}", u8::from(enabled)));
                    } else {
                        fields.push(format!(
                            "mux={}",
                            mux.as_u64()
                                .filter(|mux| *mux <= 65535)
                                .ok_or_else(outbound::invalid)?
                        ));
                    }
                }
            }
        }
        _ => {
            return Err(ParseReason::new(
                "unsupported_dependency",
                "节点需要尚未支持的代理插件",
            ));
        }
    }
    target.insert("plugin_opts".into(), Value::String(fields.join(";")));
    Ok(())
}

fn escaped(value: &str) -> String {
    let mut result = String::new();
    for c in value.chars() {
        if matches!(c, '\\' | ';' | '=' | ':') {
            result.push('\\');
        }
        result.push(c);
    }
    result
}

fn convert(value: &Value) -> Result<Value, ParseReason> {
    let source = value.as_object().ok_or_else(outbound::invalid)?;
    if source.keys().any(|key| {
        [
            "dialer-proxy",
            "interface-name",
            "routing-mark",
            "ca",
            "client-cert",
            "client-key",
            "ip-version",
        ]
        .contains(&key.as_str())
    }) {
        return Err(outbound::dependency());
    }
    let source_kind = string(source, "type")?;
    let kind = match source_kind.as_str() {
        "ss" => "shadowsocks",
        "socks5" => "socks",
        "vmess" | "vless" | "trojan" | "hysteria2" | "tuic" | "anytls" | "http" => {
            source_kind.as_str()
        }
        "naive" => {
            return Err(ParseReason::new(
                "unsupported_runtime_capability",
                "节点需要尚未确认的平台运行时能力",
            ));
        }
        _ => return Err(ParseReason::new("unsupported_protocol", "节点协议尚未支持")),
    };
    let extra: &[&str] = match kind {
        "shadowsocks" => &[
            "cipher",
            "password",
            "plugin",
            "plugin-opts",
            "udp-over-tcp",
            "udp-over-tcp-version",
        ],
        "vmess" => &[
            "uuid",
            "alterId",
            "cipher",
            "network",
            "ws-opts",
            "grpc-opts",
            "h2-opts",
            "http-opts",
            "tls",
            "servername",
            "sni",
            "skip-cert-verify",
            "alpn",
            "client-fingerprint",
            "fingerprint",
            "packet-encoding",
            "global-padding",
            "authenticated-length",
            "smux",
        ],
        "vless" => &[
            "uuid",
            "flow",
            "network",
            "ws-opts",
            "grpc-opts",
            "h2-opts",
            "http-opts",
            "tls",
            "servername",
            "sni",
            "skip-cert-verify",
            "alpn",
            "client-fingerprint",
            "fingerprint",
            "reality-opts",
            "packet-encoding",
            "smux",
        ],
        "trojan" => &[
            "password",
            "network",
            "ws-opts",
            "grpc-opts",
            "h2-opts",
            "http-opts",
            "tls",
            "servername",
            "sni",
            "skip-cert-verify",
            "alpn",
            "client-fingerprint",
            "fingerprint",
            "smux",
        ],
        "hysteria2" => &[
            "password",
            "sni",
            "skip-cert-verify",
            "alpn",
            "fingerprint",
            "client-fingerprint",
            "tls",
            "up",
            "down",
            "obfs",
            "obfs-password",
            "ports",
            "hop-interval",
            "hop-interval-max",
        ],
        "tuic" => &[
            "uuid",
            "password",
            "sni",
            "skip-cert-verify",
            "alpn",
            "fingerprint",
            "client-fingerprint",
            "tls",
            "congestion-controller",
            "udp-relay-mode",
            "udp-over-stream",
            "reduce-rtt",
            "heartbeat-interval",
            "version",
        ],
        "anytls" => &[
            "password",
            "sni",
            "skip-cert-verify",
            "alpn",
            "client-fingerprint",
            "fingerprint",
            "tls",
            "idle-session-check-interval",
            "idle-session-timeout",
            "min-idle-session",
        ],
        "socks" => &[
            "username",
            "password",
            "udp-over-tcp",
            "udp-over-tcp-version",
            "tls",
            "skip-cert-verify",
            "sni",
        ],
        "http" => &[
            "username",
            "password",
            "tls",
            "sni",
            "skip-cert-verify",
            "alpn",
            "headers",
        ],
        _ => &[],
    };
    let mut allowed = vec!["type", "name", "server", "port", "udp", "tfo", "mptcp"];
    allowed.extend_from_slice(extra);
    checked_keys(source, &allowed)?;
    let mut target = json!({"type":kind,"server":source.get("server").ok_or_else(outbound::invalid)?,"server_port":number(source.get("port").ok_or_else(outbound::invalid)?)?})
        .as_object().expect("literal object").clone();
    transfer(source, &mut target, "tfo", "tcp_fast_open");
    transfer(source, &mut target, "mptcp", "tcp_multi_path");
    if let Some(udp) = source.get("udp") {
        let enabled = udp.as_bool().ok_or_else(outbound::invalid)?;
        if !enabled && !["http", "anytls"].contains(&kind) {
            target.insert("network".into(), json!(["tcp"]));
        }
        if kind == "anytls" && !enabled {
            return Err(outbound::unsupported());
        }
    }
    if [
        "vmess",
        "vless",
        "trojan",
        "hysteria2",
        "tuic",
        "anytls",
        "http",
    ]
    .contains(&kind)
        && let Some(tls) = tls(source, kind)?
    {
        target.insert("tls".into(), tls);
    }
    if ["vmess", "vless", "trojan"].contains(&kind) {
        if let Some(transport) = transport(source)? {
            target.insert("transport".into(), transport);
        }
        transfer(source, &mut target, "packet-encoding", "packet_encoding");
        if source.contains_key("smux") {
            return Err(ParseReason::new(
                "unsupported_parameter",
                "节点的复用配置尚未支持等价转换",
            ));
        }
    }
    match kind {
        "shadowsocks" => {
            transfer(source, &mut target, "cipher", "method");
            transfer(source, &mut target, "password", "password");
            plugin(source, &mut target)?;
        }
        "vmess" => {
            transfer(source, &mut target, "uuid", "uuid");
            transfer(source, &mut target, "cipher", "security");
            if let Some(alter_id) = source.get("alterId") {
                target.insert("alter_id".into(), number(alter_id)?);
            }
            transfer(source, &mut target, "global-padding", "global_padding");
            transfer(
                source,
                &mut target,
                "authenticated-length",
                "authenticated_length",
            );
        }
        "vless" => {
            transfer(source, &mut target, "uuid", "uuid");
            transfer(source, &mut target, "flow", "flow");
        }
        "trojan" => transfer(source, &mut target, "password", "password"),
        "hysteria2" => {
            transfer(source, &mut target, "password", "password");
            for (from, to) in [("up", "up_mbps"), ("down", "down_mbps")] {
                if let Some(input) = source.get(from) {
                    target.insert(to.into(), number(input)?);
                }
            }
            if let Some(obfs) = source.get("obfs") {
                target.insert("obfs".into(), json!({"type":obfs,"password":source.get("obfs-password").ok_or_else(outbound::credential)?}));
            } else if source.contains_key("obfs-password") {
                return Err(outbound::invalid());
            }
            if let Some(ports) = source.get("ports") {
                let ports = outbound::list(ports)?
                    .iter()
                    .map(|ports| ports.replace('-', ":"))
                    .collect::<Vec<_>>();
                target.insert("server_ports".into(), json!(ports));
            }
            for (from, to) in [
                ("hop-interval", "hop_interval"),
                ("hop-interval-max", "hop_interval_max"),
            ] {
                if let Some(value) = source.get(from) {
                    let duration = if value.is_number() {
                        format!("{}s", value.as_u64().ok_or_else(outbound::invalid)?)
                    } else {
                        value.as_str().ok_or_else(outbound::invalid)?.into()
                    };
                    target.insert(to.into(), Value::String(duration));
                }
            }
        }
        "tuic" => {
            if source
                .get("version")
                .is_some_and(|version| version.as_u64() != Some(5))
            {
                return Err(ParseReason::new(
                    "unsupported_protocol",
                    "只支持 TUIC v5 节点",
                ));
            }
            transfer(source, &mut target, "uuid", "uuid");
            transfer(source, &mut target, "password", "password");
            for (from, to) in [
                ("congestion-controller", "congestion_control"),
                ("udp-relay-mode", "udp_relay_mode"),
                ("udp-over-stream", "udp_over_stream"),
                ("reduce-rtt", "zero_rtt_handshake"),
            ] {
                transfer(source, &mut target, from, to);
            }
            if let Some(heartbeat) = source.get("heartbeat-interval") {
                let duration = if heartbeat.is_number() {
                    format!("{}ms", heartbeat.as_u64().ok_or_else(outbound::invalid)?)
                } else {
                    heartbeat.as_str().ok_or_else(outbound::invalid)?.into()
                };
                target.insert("heartbeat".into(), Value::String(duration));
            }
        }
        "anytls" => {
            transfer(source, &mut target, "password", "password");
            for (from, to) in [
                ("idle-session-check-interval", "idle_session_check_interval"),
                ("idle-session-timeout", "idle_session_timeout"),
            ] {
                if let Some(value) = source.get(from) {
                    let duration = if value.is_number() {
                        format!("{}s", value.as_u64().ok_or_else(outbound::invalid)?)
                    } else {
                        value.as_str().ok_or_else(outbound::invalid)?.into()
                    };
                    target.insert(to.into(), Value::String(duration));
                }
            }
            transfer(source, &mut target, "min-idle-session", "min_idle_session");
        }
        "socks" | "http" => {
            if kind == "socks"
                && ["tls", "skip-cert-verify", "sni"]
                    .iter()
                    .any(|key| source.contains_key(*key))
            {
                return Err(ParseReason::new(
                    "unsupported_transport",
                    "SOCKS 的附加 TLS 无法等价转换",
                ));
            }
            transfer(source, &mut target, "username", "username");
            transfer(source, &mut target, "password", "password");
            if kind == "socks" {
                target.insert("version".into(), json!("5"));
            } else {
                transfer(source, &mut target, "headers", "headers");
            }
        }
        _ => return Err(outbound::unsupported()),
    }
    if source.contains_key("udp-over-tcp-version") && !source.contains_key("udp-over-tcp") {
        return Err(outbound::invalid());
    }
    if let Some(uot) = source.get("udp-over-tcp") {
        let enabled = uot.as_bool().ok_or_else(outbound::invalid)?;
        target.insert("udp_over_tcp".into(), json!({"enabled":enabled,"version":source.get("udp-over-tcp-version").cloned().unwrap_or(json!(2))}));
    }
    Ok(Value::Object(target))
}

fn rejected(value: &Value, ordinal: usize, reason: ParseReason) -> ParsedNode {
    let name = value.get("name").and_then(Value::as_str);
    let server_port = value
        .get("port")
        .and_then(|value| number(value).ok())
        .as_ref()
        .and_then(outbound::port);
    let name = super::redact_auth_name(&display_name(name, ordinal), value, ordinal);
    outbound::rejected(NodePreview {
        ordinal,
        name,
        protocol: None,
        server: value
            .get("server")
            .and_then(Value::as_str)
            .and_then(outbound::host),
        server_port,
        sni: value
            .get("sni")
            .or_else(|| value.get("servername"))
            .and_then(Value::as_str)
            .and_then(outbound::host),
        transport: None,
        parse_status: ParseStatus::Unsupported,
        unsupported_reasons: vec![reason],
    })
}

pub(super) fn parse_document(
    value: Value,
) -> Result<(Vec<ParsedNode>, Vec<ParseReason>), ParseError> {
    let object = value.as_object().ok_or_else(ParseError::document)?;
    if object.contains_key("proxies") && object.contains_key("payload") {
        return Err(ParseError::new(
            "duplicate_field",
            "订阅内容同时包含两种节点集合",
        ));
    }
    let definitions = match object.get("proxies").or_else(|| object.get("payload")) {
        Some(value) => value.as_array().ok_or_else(ParseError::document)?,
        None if object.contains_key("proxy-providers") => {
            return Err(ParseError::new(
                "provider_only",
                "配置只有订阅来源地址，请使用包含具体节点的节点集地址",
            ));
        }
        None => return Err(ParseError::document()),
    };
    if definitions.len() > MAX_NODES {
        return Err(ParseError::limit());
    }
    let mut nodes = Vec::new();
    let nonproxy = BTreeSet::from([
        "direct",
        "reject",
        "select",
        "url-test",
        "fallback",
        "load-balance",
        "relay",
    ]);
    let mut ignored = false;
    for value in definitions {
        if value
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| nonproxy.contains(kind))
        {
            ignored = true;
            continue;
        }
        let ordinal = nodes.len();
        let mut definition = value.clone();
        let provider = super::take_provider_id(&mut definition);
        let node = match convert(&definition) {
            Ok(converted) => outbound::node(
                converted,
                ordinal,
                definition.get("name").and_then(Value::as_str),
            ),
            Err(reason) => rejected(&definition, ordinal, reason),
        };
        nodes.push(outbound::attach_provider(node, provider));
    }
    let warnings = if ignored
        || object
            .keys()
            .any(|key| !["proxies", "payload"].contains(&key.as_str()))
    {
        vec![ParseReason::new(
            "global_configuration_ignored",
            "只导入具体代理节点；原全局配置与选择组未导入",
        )]
    } else {
        Vec::new()
    };
    Ok((nodes, warnings))
}

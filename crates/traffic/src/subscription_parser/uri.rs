use super::{
    MAX_BODY_BYTES, MAX_NODES, MAX_SCALAR_BYTES, NodePreview, ParseError, ParseReason, ParseStatus,
    ParsedNode, display_name, document, outbound,
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use reqwest::Url;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub(super) fn decode_base64(text: &str) -> Result<Vec<u8>, ParseError> {
    if text.len() > MAX_BODY_BYTES {
        return Err(ParseError::limit());
    }
    let compact = text
        .bytes()
        .filter(|c| !c.is_ascii_whitespace())
        .collect::<Vec<_>>();
    for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
        if let Ok(decoded) = engine.decode(&compact) {
            if decoded.len() > MAX_BODY_BYTES {
                return Err(ParseError::limit());
            }
            return Ok(decoded);
        }
    }
    Err(ParseError::document())
}

pub(super) fn percent(text: &str) -> Result<String, ParseReason> {
    let bytes = text.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'%' {
            let end = cursor
                .checked_add(3)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(outbound::invalid)?;
            let digits =
                std::str::from_utf8(&bytes[cursor + 1..end]).map_err(|_| outbound::invalid())?;
            result.push(u8::from_str_radix(digits, 16).map_err(|_| outbound::invalid())?);
            cursor = end;
        } else {
            result.push(bytes[cursor]);
            cursor += 1;
        }
    }
    String::from_utf8(result).map_err(|_| outbound::invalid())
}

fn rejected(ordinal: usize, name: Option<&str>, reason: ParseReason) -> ParsedNode {
    outbound::rejected(NodePreview {
        ordinal,
        name: display_name(name, ordinal),
        protocol: None,
        server: None,
        server_port: None,
        sni: None,
        transport: None,
        parse_status: ParseStatus::Unsupported,
        unsupported_reasons: vec![reason],
    })
}

pub(super) fn parse_list(text: &str) -> Result<Vec<ParsedNode>, ParseError> {
    let mut nodes = Vec::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        if nodes.len() >= MAX_NODES || line.len() > MAX_SCALAR_BYTES {
            return Err(ParseError::limit());
        }
        if !line.contains("://") {
            return Err(ParseError::document());
        }
        let name = line
            .rsplit_once('#')
            .and_then(|(_, fragment)| percent(fragment).ok());
        let ordinal = nodes.len();
        let node = match parse_uri(line) {
            Ok((value, parsed_name)) => {
                outbound::node(value, ordinal, parsed_name.as_deref().or(name.as_deref()))
            }
            Err(reason) => {
                let mut node = rejected(ordinal, name.as_deref(), reason);
                if let Ok(url) = Url::parse(line) {
                    let username = percent(url.username()).unwrap_or_default();
                    let password = url.password().and_then(|password| percent(password).ok());
                    let mut secrets = json!({"username":username,"password":password});
                    if url.scheme() == "ss"
                        && url.password().is_none()
                        && let Ok(bytes) = decode_base64(&username)
                        && let Ok(decoded) = String::from_utf8(bytes)
                        && let Some((_, password)) = decoded.split_once(':')
                    {
                        secrets["password"] = json!(password);
                    }
                    node.preview.name =
                        super::redact_auth_name(&node.preview.name, &secrets, ordinal);
                }
                node
            }
        };
        nodes.push(node);
    }
    Ok(nodes)
}

fn queries(url: &Url, allowed: &[&str]) -> Result<BTreeMap<String, String>, ParseReason> {
    if let Some(query) = url.query() {
        percent(query)?;
    }
    let mut result = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if !allowed.contains(&key.as_ref()) {
            return Err(outbound::unsupported());
        }
        if result
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(outbound::invalid());
        }
    }
    Ok(result)
}

fn query_bool(value: &str) -> Result<bool, ParseReason> {
    match value {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(outbound::invalid()),
    }
}

fn number(value: &str) -> Result<Value, ParseReason> {
    value
        .parse::<u64>()
        .map(Value::from)
        .map_err(|_| outbound::invalid())
}

fn endpoint(url: &Url, kind: &str) -> Result<Map<String, Value>, ParseReason> {
    if url.host_str().is_none() || !url.path().is_empty() && url.path() != "/" {
        return Err(outbound::invalid());
    }
    let server = url
        .host_str()
        .and_then(outbound::host)
        .ok_or_else(outbound::invalid)?;
    let default = match url.scheme() {
        "http" => Some(80),
        "https" | "hysteria2" | "hy2" => Some(443),
        _ => None,
    };
    let port = url
        .port()
        .or(default)
        .filter(|port| *port != 0)
        .ok_or_else(outbound::invalid)?;
    Ok(json!({"type":kind,"server":server,"server_port":port})
        .as_object()
        .expect("literal object")
        .clone())
}

fn tls_from_query(
    query: &BTreeMap<String, String>,
    mandatory: bool,
    reality_allowed: bool,
) -> Result<Option<Value>, ParseReason> {
    let mode = query
        .get("security")
        .map(String::as_str)
        .unwrap_or(if mandatory { "tls" } else { "none" });
    if !["none", "tls", "reality"].contains(&mode) || mode == "reality" && !reality_allowed {
        return Err(outbound::invalid());
    }
    let enabled = mode != "none";
    let mut tls = json!({"enabled":enabled})
        .as_object()
        .expect("literal object")
        .clone();
    let sni = query.get("sni").or_else(|| query.get("peer"));
    if query.contains_key("sni") && query.contains_key("peer") {
        return Err(outbound::invalid());
    }
    if let Some(sni) = sni {
        tls.insert("server_name".into(), Value::String(sni.clone()));
    }
    if let Some(alpn) = query.get("alpn") {
        tls.insert(
            "alpn".into(),
            Value::Array(
                alpn.split(',')
                    .map(|alpn| Value::String(alpn.into()))
                    .collect(),
            ),
        );
    }
    let insecure = query.get("allowInsecure").or_else(|| query.get("insecure"));
    if query.contains_key("allowInsecure") && query.contains_key("insecure") {
        return Err(outbound::invalid());
    }
    if let Some(insecure) = insecure {
        tls.insert("insecure".into(), Value::Bool(query_bool(insecure)?));
    }
    if let Some(fingerprint) = query.get("fp") {
        tls.insert(
            "utls".into(),
            json!({"enabled":true,"fingerprint":fingerprint}),
        );
    }
    if mode == "reality" {
        let public_key = query.get("pbk").ok_or_else(outbound::credential)?;
        tls.insert("reality".into(), json!({"enabled":true,"public_key":public_key,"short_id":query.get("sid").map(String::as_str).unwrap_or("")}));
    } else if query.contains_key("pbk") || query.contains_key("sid") {
        return Err(outbound::invalid());
    }
    if !enabled && tls.len() != 1 {
        return Err(outbound::invalid());
    }
    Ok(enabled.then_some(Value::Object(tls)))
}

fn transport_from_query(query: &BTreeMap<String, String>) -> Result<Option<Value>, ParseReason> {
    let kind = query.get("type").map(String::as_str).unwrap_or("tcp");
    let path = query.get("path").map(String::as_str).unwrap_or("");
    let host = query.get("host").map(String::as_str).unwrap_or("");
    let transport = match kind {
        "tcp" => {
            if query
                .get("headerType")
                .is_some_and(|header| header != "none")
                || ["path", "host", "serviceName", "mode", "ed", "eh"]
                    .iter()
                    .any(|key| query.contains_key(*key))
            {
                return Err(outbound::unsupported());
            }
            return Ok(None);
        }
        "ws" => {
            let mut transport = json!({"type":"ws", "path":path});
            if !host.is_empty() {
                transport["headers"] = json!({"Host":host});
            }
            if let Some(early) = query.get("ed") {
                transport["max_early_data"] = number(early)?;
            }
            if let Some(header) = query.get("eh") {
                transport["early_data_header_name"] = Value::String(header.clone());
            }
            transport
        }
        "grpc" => {
            if query
                .get("mode")
                .is_some_and(|mode| !["gun", ""].contains(&mode.as_str()))
            {
                return Err(outbound::unsupported());
            }
            json!({"type":"grpc", "service_name":query.get("serviceName").map(String::as_str).unwrap_or("")})
        }
        "http" | "h2" => {
            json!({"type":"http", "path":path,"host":host.split(',').filter(|host| !host.is_empty()).collect::<Vec<_>>()})
        }
        "httpupgrade" => json!({"type":"httpupgrade", "path":path,"host":host}),
        "quic" => json!({"type":"quic"}),
        _ => {
            return Err(ParseReason::new(
                "unsupported_transport",
                "节点的传输方式尚未支持",
            ));
        }
    };
    let meaningful = match kind {
        "ws" => ["path", "host", "ed", "eh"].as_slice(),
        "grpc" => ["serviceName", "mode"].as_slice(),
        "http" | "h2" | "httpupgrade" => ["path", "host"].as_slice(),
        _ => [].as_slice(),
    };
    if [
        "path",
        "host",
        "serviceName",
        "mode",
        "ed",
        "eh",
        "headerType",
    ]
    .iter()
    .any(|key| query.contains_key(*key) && !meaningful.contains(key))
    {
        return Err(outbound::unsupported());
    }
    Ok(Some(transport))
}

fn parse_uri(text: &str) -> Result<(Value, Option<String>), ParseReason> {
    if text.starts_with("vmess://") {
        return vmess(text);
    }
    if text.starts_with("ss://") {
        return shadowsocks(text);
    }
    let url = Url::parse(text).map_err(|_| outbound::invalid())?;
    let kind = match url.scheme() {
        "vless" | "trojan" | "tuic" | "anytls" => url.scheme(),
        "hysteria2" | "hy2" => "hysteria2",
        "socks" | "socks5" | "socks4" | "socks4a" => "socks",
        "http" | "https" => "http",
        "naive+https" | "naive" => {
            return Err(ParseReason::new(
                "unsupported_runtime_capability",
                "节点需要尚未确认的平台运行时能力",
            ));
        }
        _ => return Err(ParseReason::new("unsupported_protocol", "节点协议尚未支持")),
    };
    let allowed: &[&str] = match kind {
        "vless" => &[
            "encryption",
            "security",
            "sni",
            "peer",
            "alpn",
            "fp",
            "pbk",
            "sid",
            "flow",
            "type",
            "path",
            "host",
            "serviceName",
            "mode",
            "headerType",
            "ed",
            "eh",
            "allowInsecure",
            "insecure",
            "packetEncoding",
        ],
        "trojan" => &[
            "security",
            "sni",
            "peer",
            "alpn",
            "fp",
            "type",
            "path",
            "host",
            "serviceName",
            "mode",
            "headerType",
            "ed",
            "eh",
            "allowInsecure",
            "insecure",
        ],
        "hysteria2" => &[
            "sni",
            "alpn",
            "insecure",
            "obfs",
            "obfs-password",
            "upmbps",
            "downmbps",
            "mport",
            "hop-interval",
        ],
        "tuic" => &[
            "sni",
            "alpn",
            "allowInsecure",
            "insecure",
            "congestion_control",
            "udp_relay_mode",
            "udp_over_stream",
            "zero_rtt_handshake",
            "heartbeat",
        ],
        "anytls" => &[
            "sni",
            "alpn",
            "insecure",
            "idle_session_check_interval",
            "idle_session_timeout",
            "min_idle_session",
        ],
        "socks" => &["udp", "uot"],
        "http" => &["sni", "alpn", "insecure"],
        _ => &[],
    };
    let query = queries(&url, allowed)?;
    let mut value = endpoint(&url, kind)?;
    let username = percent(url.username())?;
    let password = url.password().map(percent).transpose()?;
    match kind {
        "vless" => {
            if password.is_some() || query.get("encryption").is_some_and(|value| value != "none") {
                return Err(outbound::invalid());
            }
            value.insert("uuid".into(), Value::String(username));
            if let Some(flow) = query.get("flow") {
                value.insert("flow".into(), Value::String(flow.clone()));
            }
            if let Some(packet) = query.get("packetEncoding") {
                value.insert("packet_encoding".into(), Value::String(packet.clone()));
            }
        }
        "trojan" | "anytls" => {
            if password.is_some() {
                return Err(outbound::credential());
            }
            value.insert("password".into(), Value::String(username));
        }
        "hysteria2" => {
            let auth = password
                .map(|password| format!("{username}:{password}"))
                .unwrap_or(username);
            value.insert("password".into(), Value::String(auth));
        }
        "tuic" => {
            value.insert("uuid".into(), Value::String(username));
            value.insert(
                "password".into(),
                Value::String(password.ok_or_else(outbound::credential)?),
            );
        }
        "socks" | "http" => {
            if !username.is_empty() {
                value.insert("username".into(), Value::String(username));
            }
            if let Some(password) = password {
                value.insert("password".into(), Value::String(password));
            }
        }
        _ => {}
    }
    match kind {
        "vless" | "trojan" => {
            if let Some(tls) = tls_from_query(&query, kind == "trojan", kind == "vless")? {
                value.insert("tls".into(), tls);
            }
            if let Some(transport) = transport_from_query(&query)? {
                value.insert("transport".into(), transport);
            }
        }
        "hysteria2" => {
            value.insert(
                "tls".into(),
                tls_from_query(&query, true, false)?.ok_or_else(outbound::invalid)?,
            );
            if let Some(obfs) = query.get("obfs") {
                value.insert("obfs".into(), json!({"type":obfs,"password":query.get("obfs-password").ok_or_else(outbound::credential)?}));
            } else if query.contains_key("obfs-password") {
                return Err(outbound::invalid());
            }
            for (from, to) in [("upmbps", "up_mbps"), ("downmbps", "down_mbps")] {
                if let Some(input) = query.get(from) {
                    value.insert(to.into(), number(input)?);
                }
            }
            if let Some(ports) = query.get("mport") {
                value.insert("server_ports".into(), json!([ports.replace('-', ":")]));
            }
            if let Some(interval) = query.get("hop-interval") {
                value.insert("hop_interval".into(), Value::String(interval.clone()));
            }
        }
        "tuic" => {
            value.insert(
                "tls".into(),
                tls_from_query(&query, true, false)?.ok_or_else(outbound::invalid)?,
            );
            for key in ["congestion_control", "udp_relay_mode", "heartbeat"] {
                if let Some(input) = query.get(key) {
                    value.insert(key.into(), Value::String(input.clone()));
                }
            }
            for key in ["udp_over_stream", "zero_rtt_handshake"] {
                if let Some(input) = query.get(key) {
                    value.insert(key.into(), Value::Bool(query_bool(input)?));
                }
            }
        }
        "anytls" => {
            value.insert(
                "tls".into(),
                tls_from_query(&query, true, false)?.ok_or_else(outbound::invalid)?,
            );
            for key in ["idle_session_check_interval", "idle_session_timeout"] {
                if let Some(input) = query.get(key) {
                    value.insert(key.into(), Value::String(input.clone()));
                }
            }
            if let Some(input) = query.get("min_idle_session") {
                value.insert("min_idle_session".into(), number(input)?);
            }
        }
        "socks" => {
            let version = match url.scheme() {
                "socks4" => "4",
                "socks4a" => "4a",
                _ => "5",
            };
            value.insert("version".into(), Value::String(version.into()));
            if let Some(input) = query.get("udp")
                && !query_bool(input)?
            {
                value.insert("network".into(), json!(["tcp"]));
            }
            if let Some(input) = query.get("uot") {
                value.insert("udp_over_tcp".into(), Value::Bool(query_bool(input)?));
            }
        }
        "http" => {
            if let Some(tls) = tls_from_query(&query, url.scheme() == "https", false)? {
                value.insert("tls".into(), tls);
            }
        }
        _ => {}
    }
    let name = url.fragment().map(percent).transpose()?;
    Ok((Value::Object(value), name))
}

fn shadowsocks(text: &str) -> Result<(Value, Option<String>), ParseReason> {
    let mut decoded_legacy = None;
    let main = text
        .strip_prefix("ss://")
        .ok_or_else(outbound::invalid)?
        .split(['?', '#'])
        .next()
        .ok_or_else(outbound::invalid)?;
    let encoded_legacy = !main.contains('@');
    if encoded_legacy {
        let decoded = decode_base64(main).map_err(|_| outbound::invalid())?;
        let decoded = String::from_utf8(decoded).map_err(|_| outbound::invalid())?;
        let suffix = &text[5 + main.len()..];
        decoded_legacy = Some(format!("ss://{decoded}{suffix}"));
    }
    let url =
        Url::parse(decoded_legacy.as_deref().unwrap_or(text)).map_err(|_| outbound::invalid())?;
    let query = queries(&url, &["plugin"])?;
    let mut value = endpoint(&url, "shadowsocks")?;
    let (method, password, base64_userinfo) = if let Some(password) = url.password() {
        (percent(url.username())?, percent(password)?, false)
    } else {
        let user = percent(url.username())?;
        let decoded = decode_base64(&user).map_err(|_| outbound::credential())?;
        let decoded = String::from_utf8(decoded).map_err(|_| outbound::credential())?;
        let (method, password) = decoded.split_once(':').ok_or_else(outbound::credential)?;
        (method.to_owned(), password.to_owned(), true)
    };
    if method.starts_with("2022-") && (base64_userinfo || encoded_legacy) {
        return Err(outbound::credential());
    }
    value.insert("method".into(), Value::String(method));
    value.insert("password".into(), Value::String(password));
    if let Some(plugin) = query.get("plugin") {
        let (name, options) = plugin.split_once(';').unwrap_or((plugin, ""));
        value.insert("plugin".into(), Value::String(name.into()));
        value.insert("plugin_opts".into(), Value::String(options.into()));
    }
    Ok((
        Value::Object(value),
        url.fragment().map(percent).transpose()?,
    ))
}

fn legacy_number(value: Option<&Value>, default: u64) -> Result<Value, ParseReason> {
    match value {
        None => Ok(default.into()),
        Some(Value::String(value)) => number(value),
        Some(value) if value.as_u64().is_some() => Ok(value.clone()),
        _ => Err(outbound::invalid()),
    }
}

fn legacy_string<'a>(
    value: &'a Map<String, Value>,
    key: &str,
    default: &'a str,
) -> Result<&'a str, ParseReason> {
    value
        .get(key)
        .map(|value| value.as_str().ok_or_else(outbound::invalid))
        .unwrap_or(Ok(default))
}

fn vmess(text: &str) -> Result<(Value, Option<String>), ParseReason> {
    let encoded = text
        .strip_prefix("vmess://")
        .ok_or_else(outbound::invalid)?;
    if encoded.contains(['?', '#']) {
        return Err(outbound::unsupported());
    }
    let decoded = decode_base64(encoded).map_err(|_| outbound::invalid())?;
    let decoded = std::str::from_utf8(&decoded).map_err(|_| outbound::invalid())?;
    let doc = document::json(decoded).map_err(|_| outbound::invalid())?;
    let doc = doc.as_object().ok_or_else(outbound::invalid)?;
    if doc.keys().any(|key| {
        ![
            "v",
            "ps",
            "add",
            "port",
            "id",
            "aid",
            "scy",
            "net",
            "type",
            "host",
            "path",
            "tls",
            "sni",
            "alpn",
            "fp",
            "allowInsecure",
        ]
        .contains(&key.as_str())
    }) {
        return Err(outbound::unsupported());
    }
    if !["", "2"].contains(&legacy_string(doc, "v", "2")?)
        || !["", "none"].contains(&legacy_string(doc, "type", "none")?)
    {
        return Err(outbound::unsupported());
    }
    let mut value = json!({"type":"vmess", "server":legacy_string(doc,"add","")?, "server_port":legacy_number(doc.get("port"),0)?,
        "uuid":legacy_string(doc,"id","")?, "alter_id":legacy_number(doc.get("aid"),0)?, "security":legacy_string(doc,"scy","auto")?})
        .as_object().expect("literal object").clone();
    let mut query = BTreeMap::new();
    query.insert("type".into(), legacy_string(doc, "net", "tcp")?.into());
    for (from, to) in [
        ("host", "host"),
        ("path", "path"),
        ("sni", "sni"),
        ("alpn", "alpn"),
        ("fp", "fp"),
    ] {
        let content = legacy_string(doc, from, "")?;
        if !content.is_empty() {
            query.insert(to.into(), content.into());
        }
    }
    if query.get("type").is_some_and(|kind| kind == "grpc") {
        if let Some(path) = query.remove("path") {
            query.insert("serviceName".into(), path);
        }
        if query.contains_key("host") {
            return Err(outbound::unsupported());
        }
    }
    if let Some(insecure) = doc.get("allowInsecure") {
        let insecure = insecure.as_bool().ok_or_else(outbound::invalid)?;
        query.insert("allowInsecure".into(), insecure.to_string());
    }
    let tls = legacy_string(doc, "tls", "")?;
    if !["", "none", "tls"].contains(&tls) {
        return Err(outbound::unsupported());
    }
    query.insert(
        "security".into(),
        if tls == "tls" { "tls" } else { "none" }.into(),
    );
    if let Some(tls) = tls_from_query(&query, false, false)? {
        value.insert("tls".into(), tls);
    }
    if let Some(transport) = transport_from_query(&query)? {
        value.insert("transport".into(), transport);
    }
    let name = doc
        .get("ps")
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(outbound::invalid)
        })
        .transpose()?;
    Ok((Value::Object(value), name))
}

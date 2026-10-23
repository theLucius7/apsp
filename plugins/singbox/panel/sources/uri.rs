use super::parse::{ImportError, decode_base64};
use apsp_compiler::external::ExternalOutbound;
use reqwest::Url;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub(super) fn parse(line: &str) -> Result<(ExternalOutbound, String), ImportError> {
    percent_decode(line)?;
    if let Some(encoded) = line.strip_prefix("vmess://") {
        return vmess(encoded);
    }
    if line.starts_with("ss://") {
        return shadowsocks(line);
    }
    let url = Url::parse(line).map_err(|_| ImportError("invalid_proxy_uri"))?;
    let mut params = parameters(&url)?;
    let protocol = match url.scheme() {
        "vless" | "trojan" | "tuic" | "anytls" => url.scheme(),
        "hysteria2" | "hy2" => "hysteria2",
        "http" | "https" => "http",
        "socks" | "socks5" | "socks5h" | "socks4" | "socks4a" => "socks",
        _ => return Err(ImportError("unsupported_proxy_protocol")),
    };
    if !["", "/"].contains(&url.path()) {
        return Err(ImportError("unsupported_uri_path"));
    }
    let mut output = endpoint(&url, protocol)?;
    let user = percent_decode(url.username())?;
    let password = url.password().map(percent_decode).transpose()?;
    match protocol {
        "vless" => {
            if password.is_some() {
                return Err(ImportError("invalid_proxy_authentication"));
            }
            output["uuid"] = json!(user);
            if params.remove("encryption").is_some_and(|v| v != "none") {
                return Err(ImportError("unsupported_vless_encryption"));
            }
            move_string(&mut params, "flow", &mut output, "flow");
            move_string(
                &mut params,
                "packetEncoding",
                &mut output,
                "packet_encoding",
            );
        }
        "tuic" => {
            output["uuid"] = json!(user);
            output["password"] =
                json!(password.ok_or(ImportError("invalid_proxy_authentication"))?);
            move_string(
                &mut params,
                "congestion_control",
                &mut output,
                "congestion_control",
            );
            move_string(&mut params, "udp_relay_mode", &mut output, "udp_relay_mode");
            for key in ["udp_over_stream", "zero_rtt_handshake"] {
                move_bool(&mut params, key, &mut output, key)?;
            }
            move_string(&mut params, "heartbeat", &mut output, "heartbeat");
        }
        "trojan" | "hysteria2" | "anytls" => {
            let auth = if let Some(password) = password {
                format!("{user}:{password}")
            } else {
                user
            };
            output["password"] = json!(auth);
            if protocol == "hysteria2"
                && let Some(kind) = params.remove("obfs")
            {
                let password = params
                    .remove("obfs-password")
                    .ok_or(ImportError("invalid_obfuscation"))?;
                output["obfs"] = json!({"type":kind,"password":password});
            }
        }
        "http" | "socks" => {
            if !user.is_empty() {
                output["username"] = json!(user);
            }
            if let Some(password) = password {
                output["password"] = json!(password);
            }
            if protocol == "socks" {
                output["version"] = json!(match url.scheme() {
                    "socks4" => "4",
                    "socks4a" => "4a",
                    _ => "5",
                });
                move_bool(&mut params, "uot", &mut output, "udp_over_tcp")?;
            }
        }
        _ => return Err(ImportError("unsupported_proxy_protocol")),
    }
    move_string(&mut params, "network", &mut output, "network");
    let tls_default =
        ["trojan", "hysteria2", "tuic", "anytls"].contains(&protocol) || url.scheme() == "https";
    tls(&mut output, &mut params, tls_default)?;
    transport(&mut output, &mut params)?;
    if !params.is_empty() {
        return Err(ImportError("unsupported_uri_parameter"));
    }
    finish(
        output,
        url.fragment()
            .map(percent_decode)
            .transpose()?
            .unwrap_or_default(),
    )
}

fn endpoint(url: &Url, protocol: &str) -> Result<Value, ImportError> {
    let host = url
        .host_str()
        .ok_or(ImportError("invalid_proxy_endpoint"))?
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    let port = url
        .port_or_known_default()
        .ok_or(ImportError("missing_proxy_port"))?;
    Ok(json!({"type":protocol,"server":host,"server_port":port}))
}

fn finish(output: Value, name: String) -> Result<(ExternalOutbound, String), ImportError> {
    let outbound = ExternalOutbound(output);
    outbound
        .validate()
        .map_err(|_| ImportError("unsupported_or_invalid_proxy_parameter"))?;
    Ok((outbound, name))
}

fn shadowsocks(line: &str) -> Result<(ExternalOutbound, String), ImportError> {
    let uri = line.strip_prefix("ss://").expect("prefix");
    let (main, fragment) = uri.split_once('#').unwrap_or((uri, ""));
    let expanded;
    let legacy = !main.contains('@');
    let line = if legacy {
        if main.contains('?') {
            return Err(ImportError("invalid_ss_uri"));
        }
        let decoded = decode_base64(main)?;
        let decoded = std::str::from_utf8(&decoded).map_err(|_| ImportError("invalid_ss_uri"))?;
        // Legacy encoding wraps the complete method, password, and endpoint.
        expanded = format!("ss://{decoded}#{fragment}");
        &expanded
    } else {
        line
    };
    let url = Url::parse(line).map_err(|_| ImportError("invalid_ss_uri"))?;
    let mut output = endpoint(&url, "shadowsocks")?;
    let (method, password) = if let Some(password) = url.password() {
        (percent_decode(url.username())?, percent_decode(password)?)
    } else {
        let decoded = decode_base64(&percent_decode(url.username())?)?;
        let decoded = std::str::from_utf8(&decoded).map_err(|_| ImportError("invalid_ss_uri"))?;
        let (method, password) = decoded
            .split_once(':')
            .ok_or(ImportError("invalid_ss_uri"))?;
        // SIP002 requires unencoded userinfo for AEAD-2022.
        if method.starts_with("2022-") {
            return Err(ImportError("ss2022_requires_plain_userinfo"));
        }
        (method.into(), password.into())
    };
    if legacy && method.starts_with("2022-") {
        return Err(ImportError("ss2022_requires_plain_userinfo"));
    }
    output["method"] = json!(method);
    output["password"] = json!(password);
    let mut params = parameters(&url)?;
    if params.contains_key("plugin") {
        return Err(ImportError("unsupported_ss_plugin"));
    }
    move_bool(&mut params, "uot", &mut output, "udp_over_tcp")?;
    if !params.is_empty() || !["", "/"].contains(&url.path()) {
        return Err(ImportError("unsupported_uri_parameter"));
    }
    finish(
        output,
        url.fragment()
            .map(percent_decode)
            .transpose()?
            .unwrap_or_default(),
    )
}

fn vmess(encoded: &str) -> Result<(ExternalOutbound, String), ImportError> {
    let bytes = decode_base64(encoded)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| ImportError("invalid_vmess_uri"))?;
    let value = super::structured::json(text)?;
    let input = value.as_object().ok_or(ImportError("invalid_vmess_uri"))?;
    if input.keys().any(|key| {
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
            "packetEncoding",
            "ed",
            "eh",
        ]
        .contains(&key.as_str())
    }) {
        return Err(ImportError("unsupported_vmess_parameter"));
    }
    let field = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or_default();
    if input.iter().any(|(key, value)| {
        !["v", "port", "aid", "allowInsecure", "ed"].contains(&key.as_str()) && !value.is_string()
    }) {
        return Err(ImportError("invalid_vmess_parameter"));
    }
    if input.get("v").is_some_and(|v| v != "2" && v != 2) {
        return Err(ImportError("unsupported_vmess_version"));
    }
    if !["", "none"].contains(&field("type")) {
        return Err(ImportError("unsupported_vmess_header"));
    }
    let port = numeric(input, "port", None)?;
    let mut output = json!({"type":"vmess","server":field("add").trim_matches(['[',']']).to_ascii_lowercase(),"server_port":port,"uuid":field("id"),"security":if field("scy").is_empty(){"auto"}else{field("scy")},"alter_id":numeric(input,"aid",Some(0))?});
    let mut params = BTreeMap::new();
    for (source, destination) in [
        ("net", "type"),
        ("host", "host"),
        ("path", "path"),
        ("sni", "sni"),
        ("alpn", "alpn"),
        ("fp", "fp"),
        ("packetEncoding", "packetEncoding"),
        ("eh", "eh"),
    ] {
        if !field(source).is_empty() {
            params.insert(destination.into(), field(source).into());
        }
    }
    if input.contains_key("ed") {
        params.insert("ed".into(), numeric(input, "ed", None)?.to_string());
    }
    if let Some(value) = input.get("allowInsecure") {
        let value = match value {
            Value::Bool(v) => v.to_string(),
            Value::String(v) => v.clone(),
            Value::Number(v) => v.to_string(),
            _ => return Err(ImportError("invalid_vmess_parameter")),
        };
        params.insert("allowInsecure".into(), value);
    }
    if !["", "tls", "none"].contains(&field("tls")) {
        return Err(ImportError("unsupported_vmess_tls"));
    }
    tls(&mut output, &mut params, field("tls") == "tls")?;
    transport(&mut output, &mut params)?;
    move_string(
        &mut params,
        "packetEncoding",
        &mut output,
        "packet_encoding",
    );
    if !params.is_empty() {
        return Err(ImportError("unsupported_vmess_parameter"));
    }
    finish(output, field("ps").into())
}

fn numeric(
    input: &Map<String, Value>,
    key: &str,
    default: Option<u64>,
) -> Result<u64, ImportError> {
    input
        .get(key)
        .map(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
                .ok_or(ImportError("invalid_vmess_number"))
        })
        .unwrap_or_else(|| default.ok_or(ImportError("invalid_vmess_number")))
}

fn parameters(url: &Url) -> Result<BTreeMap<String, String>, ImportError> {
    let mut params = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if params
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(ImportError("duplicate_uri_parameter"));
        }
    }
    Ok(params)
}

fn alias(
    params: &mut BTreeMap<String, String>,
    keys: &[&str],
) -> Result<Option<String>, ImportError> {
    let mut result = None;
    for key in keys {
        if let Some(value) = params.remove(*key)
            && result.replace(value).is_some()
        {
            return Err(ImportError("duplicate_uri_parameter"));
        }
    }
    Ok(result)
}

fn tls(
    output: &mut Value,
    params: &mut BTreeMap<String, String>,
    default: bool,
) -> Result<(), ImportError> {
    let security = params.remove("security");
    let enabled = match security.as_deref() {
        Some("tls" | "reality") => true,
        Some("none" | "") => false,
        None => default,
        _ => return Err(ImportError("unsupported_uri_tls")),
    };
    let sni = alias(params, &["sni", "peer", "serverName"])?;
    let insecure = alias(params, &["insecure", "allowInsecure", "allow_insecure"])?;
    let alpn = params.remove("alpn");
    let fingerprint = params.remove("fp");
    let disable_sni = params.remove("disable_sni");
    if !enabled {
        if sni.is_some()
            || insecure.is_some()
            || alpn.is_some()
            || fingerprint.is_some()
            || disable_sni.is_some()
        {
            return Err(ImportError("tls_parameters_without_tls"));
        }
        return Ok(());
    }
    let mut tls = json!({"enabled":true});
    if let Some(sni) = sni {
        tls["server_name"] = json!(sni);
    }
    if let Some(insecure) = insecure {
        tls["insecure"] = json!(bool_value(&insecure)?);
    }
    if let Some(disable_sni) = disable_sni {
        tls["disable_sni"] = json!(bool_value(&disable_sni)?);
    }
    if let Some(alpn) = alpn {
        tls["alpn"] = json!(alpn.split(',').collect::<Vec<_>>());
    }
    if let Some(fingerprint) = fingerprint {
        tls["utls"] = json!({"enabled":true,"fingerprint":fingerprint});
    }
    if security.as_deref() == Some("reality") {
        let public_key = params
            .remove("pbk")
            .ok_or(ImportError("missing_reality_key"))?;
        let short_id = params.remove("sid").unwrap_or_default();
        tls["reality"] = json!({"enabled":true,"public_key":public_key,"short_id":short_id});
    }
    output["tls"] = tls;
    Ok(())
}

fn transport(output: &mut Value, params: &mut BTreeMap<String, String>) -> Result<(), ImportError> {
    let kind = params.remove("type").unwrap_or_else(|| "tcp".into());
    let tls_enabled = output.pointer("/tls/enabled").and_then(Value::as_bool) == Some(true);
    if kind == "h2" && !tls_enabled {
        return Err(ImportError("unsupported_h2_without_tls"));
    }
    if kind == "http" && tls_enabled {
        return Err(ImportError("unsupported_http_with_tls"));
    }
    let mut transport = match kind.as_str() {
        "" | "tcp" => return Ok(()),
        "ws" | "grpc" | "httpupgrade" | "quic" => json!({"type":kind}),
        "http" | "h2" => json!({"type":"http"}),
        _ => return Err(ImportError("unsupported_proxy_transport")),
    };
    match kind.as_str() {
        "ws" | "http" | "h2" | "httpupgrade" => {
            move_string(params, "path", &mut transport, "path");
            if let Some(host) = params.remove("host") {
                if kind == "ws" {
                    transport["headers"] = json!({"Host":host});
                } else if kind == "httpupgrade" {
                    transport["host"] = json!(host);
                } else {
                    transport["host"] = json!(host.split(',').collect::<Vec<_>>());
                }
            }
        }
        "grpc" => move_string(params, "serviceName", &mut transport, "service_name"),
        _ => {}
    }
    if kind == "ws" {
        if let Some(value) = params.remove("ed") {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(ImportError("invalid_websocket_early_data"));
            }
            let size = value
                .parse::<u16>()
                .map_err(|_| ImportError("invalid_websocket_early_data"))?;
            transport["max_early_data"] = json!(size);
        }
        if let Some(header) = params.remove("eh") {
            reqwest::header::HeaderName::from_bytes(header.as_bytes())
                .map_err(|_| ImportError("invalid_websocket_early_data_header"))?;
            if header.len() > 128
                || transport["max_early_data"].as_u64().unwrap_or_default() == 0
                || [
                    "host",
                    "connection",
                    "upgrade",
                    "content-length",
                    "transfer-encoding",
                    "sec-websocket-key",
                    "sec-websocket-version",
                ]
                .contains(&header.to_ascii_lowercase().as_str())
            {
                return Err(ImportError("invalid_websocket_early_data_header"));
            }
            transport["early_data_header_name"] = json!(header);
        }
    }
    output["transport"] = transport;
    Ok(())
}

fn move_string(
    params: &mut BTreeMap<String, String>,
    source: &str,
    output: &mut Value,
    destination: &str,
) {
    if let Some(value) = params.remove(source) {
        output[destination] = json!(value);
    }
}
fn move_bool(
    params: &mut BTreeMap<String, String>,
    source: &str,
    output: &mut Value,
    destination: &str,
) -> Result<(), ImportError> {
    if let Some(value) = params.remove(source) {
        output[destination] = json!(bool_value(&value)?);
    }
    Ok(())
}
fn bool_value(value: &str) -> Result<bool, ImportError> {
    match value {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(ImportError("invalid_uri_boolean")),
    }
}

pub(super) fn percent_decode(value: &str) -> Result<String, ImportError> {
    let mut result = Vec::new();
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes
                .next()
                .and_then(|v| (v as char).to_digit(16))
                .ok_or(ImportError("invalid_uri_escape"))?;
            let low = bytes
                .next()
                .and_then(|v| (v as char).to_digit(16))
                .ok_or(ImportError("invalid_uri_escape"))?;
            result.push((high * 16 + low) as u8);
        } else {
            result.push(byte);
        }
    }
    String::from_utf8(result).map_err(|_| ImportError("invalid_uri_escape"))
}

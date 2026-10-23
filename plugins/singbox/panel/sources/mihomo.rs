use super::parse::{ImportError, provider_id};
use apsp_compiler::external::ExternalOutbound;
use serde_json::{Map, Value, json};

pub(super) fn convert(value: Value) -> Result<(ExternalOutbound, Option<String>), ImportError> {
    let mut input = value
        .as_object()
        .cloned()
        .ok_or(ImportError("invalid_node_object"))?;
    input.remove("name");
    let provider = provider_id(input.remove("provider_id"))?;
    let kind = required_text(&mut input, "type")?;
    let protocol = match kind.as_str() {
        "ss" => "shadowsocks",
        "socks5" => "socks",
        "http" | "vmess" | "vless" | "trojan" | "hysteria2" | "tuic" | "anytls" => &kind,
        _ => return Err(ImportError("unsupported_proxy_protocol")),
    };
    let server = required_text(&mut input, "server")?
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    let port = input
        .remove("port")
        .ok_or(ImportError("missing_proxy_port"))?;
    let mut output = json!({"type":protocol,"server":server,"server_port":port});
    for key in ["username", "password", "uuid"] {
        transfer(&mut input, key, &mut output, key);
    }
    if protocol == "shadowsocks" {
        transfer(&mut input, "cipher", &mut output, "method");
        transfer(&mut input, "udp-over-tcp", &mut output, "udp_over_tcp");
        if input.contains_key("udp-over-tcp-version") {
            let enabled = output
                .as_object_mut()
                .expect("object")
                .remove("udp_over_tcp")
                .unwrap_or(json!(true));
            output["udp_over_tcp"] =
                json!({"enabled":enabled,"version":input.remove("udp-over-tcp-version")});
        }
    }
    if protocol == "socks" {
        output["version"] = json!("5");
    }
    if protocol == "vmess" {
        transfer(&mut input, "cipher", &mut output, "security");
        if output.get("security").is_none() {
            output["security"] = json!("auto");
        }
        transfer(&mut input, "alterId", &mut output, "alter_id");
        if let Some(value) = input.remove("global-padding") {
            output["global_padding"] = value;
        }
        if let Some(value) = input.remove("authenticated-length") {
            output["authenticated_length"] = value;
        }
    }
    if protocol == "vless" {
        transfer(&mut input, "flow", &mut output, "flow");
    }
    if ["vless", "vmess"].contains(&protocol) {
        transfer(
            &mut input,
            "packet-encoding",
            &mut output,
            "packet_encoding",
        );
        if let Some(value) = input.remove("xudp") {
            if !value.is_boolean() || output.get("packet_encoding").is_some() {
                return Err(ImportError("invalid_packet_encoding"));
            }
            if value == true {
                output["packet_encoding"] = json!("xudp");
            }
        }
    }
    if protocol == "hysteria2" {
        for (key, destination) in [("up", "up_mbps"), ("down", "down_mbps")] {
            if let Some(value) = input.remove(key) {
                output[destination] = json!(bandwidth(value)?);
            }
        }
        if let Some(kind) = input.remove("obfs") {
            output["obfs"] = json!({"type":kind,"password":input.remove("obfs-password").ok_or(ImportError("invalid_obfuscation"))?});
        }
    }
    if protocol == "tuic" {
        for (key, destination) in [
            ("congestion-controller", "congestion_control"),
            ("udp-relay-mode", "udp_relay_mode"),
            ("udp-over-stream", "udp_over_stream"),
            ("reduce-rtt", "zero_rtt_handshake"),
        ] {
            transfer(&mut input, key, &mut output, destination);
        }
        if let Some(value) = input.remove("heartbeat-interval") {
            output["heartbeat"] = json!(duration(value, "ms")?);
        }
    }
    if protocol == "anytls" {
        for key in ["idle-session-check-interval", "idle-session-timeout"] {
            if let Some(value) = input.remove(key) {
                output[key.replace('-', "_")] = json!(duration(value, "s")?);
            }
        }
        transfer(
            &mut input,
            "min-idle-session",
            &mut output,
            "min_idle_session",
        );
    }
    if let Some(udp) = input.remove("udp") {
        if !udp.is_boolean() {
            return Err(ImportError("invalid_udp_option"));
        }
        if udp == false {
            if ["http", "anytls"].contains(&protocol) {
                return Err(ImportError("unsupported_udp_restriction"));
            }
            output["network"] = json!("tcp");
        }
    }
    let tls_default = ["trojan", "hysteria2", "tuic", "anytls"].contains(&protocol);
    tls(&mut input, &mut output, tls_default)?;
    transport(&mut input, &mut output)?;
    if let Some(mux) = input.remove("smux") {
        output["multiplex"] = mux;
    }
    // These disabled options have exactly the runtime default semantics.
    for key in ["tfo", "mptcp"] {
        if let Some(value) = input.remove(key)
            && value != false
        {
            return Err(ImportError("unsupported_socket_option"));
        }
    }
    if !input.is_empty() {
        return Err(ImportError("unsupported_mihomo_parameter"));
    }
    let outbound = ExternalOutbound(output);
    outbound
        .validate()
        .map_err(|_| ImportError("unsupported_or_invalid_proxy_parameter"))?;
    Ok((outbound, provider))
}

fn tls(
    input: &mut Map<String, Value>,
    output: &mut Value,
    default: bool,
) -> Result<(), ImportError> {
    let enabled = match input.remove("tls") {
        None => default,
        Some(Value::Bool(value)) => value,
        Some(_) => return Err(ImportError("invalid_tls_option")),
    };
    let mut tls = json!({"enabled":enabled});
    for (key, destination) in [
        ("servername", "server_name"),
        ("sni", "server_name"),
        ("skip-cert-verify", "insecure"),
        ("alpn", "alpn"),
        ("disable-sni", "disable_sni"),
    ] {
        if let Some(value) = input.remove(key) {
            if tls.get(destination).is_some() {
                return Err(ImportError("duplicate_tls_parameter"));
            }
            tls[destination] = value;
        }
    }
    if let Some(fingerprint) = input.remove("client-fingerprint") {
        tls["utls"] = json!({"enabled":true,"fingerprint":fingerprint});
    }
    if let Some(value) = input.remove("reality-opts") {
        let mut value = value
            .as_object()
            .cloned()
            .ok_or(ImportError("invalid_reality_options"))?;
        let key = value
            .remove("public-key")
            .ok_or(ImportError("missing_reality_key"))?;
        let short = value.remove("short-id").unwrap_or(json!(""));
        if !value.is_empty() {
            return Err(ImportError("unsupported_reality_parameter"));
        }
        tls["reality"] = json!({"enabled":true,"public_key":key,"short_id":short});
    }
    if !enabled && tls.as_object().expect("object").len() > 1 {
        return Err(ImportError("tls_parameters_without_tls"));
    }
    if enabled {
        output["tls"] = tls;
    }
    Ok(())
}

fn transport(input: &mut Map<String, Value>, output: &mut Value) -> Result<(), ImportError> {
    let network = input.remove("network").unwrap_or(json!("tcp"));
    let network = network
        .as_str()
        .ok_or(ImportError("invalid_proxy_transport"))?;
    let (kind, key) = match network {
        "tcp" => return Ok(()),
        "ws" => ("ws", "ws-opts"),
        "grpc" => ("grpc", "grpc-opts"),
        "h2" => ("http", "h2-opts"),
        "http" => ("http", "http-opts"),
        "httpupgrade" => ("httpupgrade", "http-upgrade-opts"),
        _ => return Err(ImportError("unsupported_proxy_transport")),
    };
    // The fixed sing-box HTTP transport chooses H1 without TLS and H2 with
    // TLS. Mihomo keeps its HTTP and H2 transports distinct in both cases.
    let tls_enabled = output.pointer("/tls/enabled").and_then(Value::as_bool) == Some(true);
    if network == "h2" && !tls_enabled {
        return Err(ImportError("unsupported_h2_without_tls"));
    }
    if network == "http" && tls_enabled {
        return Err(ImportError("unsupported_http_with_tls"));
    }
    if network == "h2" {
        // Mihomo's H2 branch overrides any configured ALPN before TLS dialing.
        // Reject malformed source parameters before replacing an ignored value.
        ExternalOutbound(output.clone())
            .validate()
            .map_err(|_| ImportError("unsupported_or_invalid_proxy_parameter"))?;
        output["tls"]["alpn"] = json!(["h2"]);
    }
    let value = input.remove(key).unwrap_or(json!({}));
    let mut params = value
        .as_object()
        .cloned()
        .ok_or(ImportError("invalid_transport_options"))?;
    let mut transport = json!({"type":kind});
    if kind == "grpc" {
        transfer(
            &mut params,
            "grpc-service-name",
            &mut transport,
            "service_name",
        );
    } else {
        if network == "http"
            && params.contains_key("host")
            && params
                .get("headers")
                .and_then(Value::as_object)
                .is_some_and(|headers| headers.keys().any(|key| key.eq_ignore_ascii_case("Host")))
        {
            return Err(ImportError("duplicate_http_host_parameter"));
        }
        let keys: &[&str] = match network {
            "h2" => &["path", "host"],
            "http" => &["path", "headers", "method"],
            _ => &["path", "host", "headers", "method"],
        };
        for key in keys {
            transfer(&mut params, key, &mut transport, key);
        }
        if network == "http" && transport.get("method").is_none_or(|value| value == "") {
            // Mihomo's HTTP camouflage defaults to GET; sing-box defaults to PUT.
            transport["method"] = json!("GET");
        }
        if network == "http"
            && let Some(mut headers) = transport.as_object_mut().expect("object").remove("headers")
        {
            let map = headers
                .as_object_mut()
                .ok_or(ImportError("invalid_http_headers"))?;
            let host_keys: Vec<_> = map
                .keys()
                .filter(|key| key.eq_ignore_ascii_case("Host"))
                .cloned()
                .collect();
            if host_keys.len() > 1 {
                return Err(ImportError("duplicate_http_host_parameter"));
            }
            if host_keys.first().is_some_and(|key| key != "Host") {
                return Err(ImportError("unsupported_http_host_header"));
            }
            if let Some(host) = map.remove("Host") {
                // Request.Host is separate from Header["Host"] in sing-box.
                transport["host"] = host;
            }
            if map
                .values()
                .any(|value| value.as_array().is_some_and(|values| values.len() > 1))
            {
                return Err(ImportError("unsupported_random_http_headers"));
            }
            transport["headers"] = headers;
        }
        if network == "http"
            && let Some(value) = transport.get("path").and_then(Value::as_array)
        {
            if value.len() != 1 {
                return Err(ImportError("unsupported_random_http_paths"));
            }
            transport["path"] = value[0].clone();
        }
        if kind == "ws" {
            transfer(
                &mut params,
                "max-early-data",
                &mut transport,
                "max_early_data",
            );
            transfer(
                &mut params,
                "early-data-header-name",
                &mut transport,
                "early_data_header_name",
            );
        }
    }
    if !params.is_empty() {
        return Err(ImportError("unsupported_transport_parameter"));
    }
    output["transport"] = transport;
    Ok(())
}

fn bandwidth(value: Value) -> Result<u64, ImportError> {
    if let Some(value) = value.as_u64() {
        return Ok(value);
    }
    let value = value
        .as_str()
        .ok_or(ImportError("invalid_bandwidth"))?
        .trim();
    let value = value.strip_suffix("Mbps").unwrap_or(value).trim();
    value
        .parse()
        .map_err(|_| ImportError("unsupported_bandwidth_unit"))
}

fn duration(value: Value, unit: &str) -> Result<String, ImportError> {
    let value = value.as_u64().ok_or(ImportError("invalid_duration"))?;
    if value > 1_000_000 {
        return Err(ImportError("invalid_duration"));
    }
    Ok(format!("{value}{unit}"))
}

fn required_text(input: &mut Map<String, Value>, key: &str) -> Result<String, ImportError> {
    match input.remove(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value),
        _ => Err(ImportError("invalid_proxy_text_parameter")),
    }
}
fn transfer(input: &mut Map<String, Value>, key: &str, output: &mut Value, destination: &str) {
    if let Some(value) = input.remove(key) {
        output[destination] = value;
    }
}

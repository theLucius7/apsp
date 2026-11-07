//! Standards-based links only; formats without a lossless URI require JSON.
use crate::{CompileError, percent_encode};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde_json::{Value, json};

pub(crate) fn render(out: &Value, name: &str, id: i64) -> Result<String, CompileError> {
    let kind = out["type"].as_str().unwrap_or("");
    let unsupported = || CompileError::RequiresJson {
        node_id: id,
        protocol: kind.into(),
    };
    if out.get("apsp_shadowtls").is_some()
        || out.get("realm").is_some()
        || out["tls"].get("certificate").is_some()
    {
        return Err(unsupported());
    }
    if [
        "multiplex",
        "udp_over_tcp",
        "udp_over_stream",
        "heartbeat",
        "zero_rtt_handshake",
        "up_mbps",
        "down_mbps",
        "bbr_profile",
        "idle_session_check_interval",
        "idle_session_timeout",
        "min_idle_session",
    ]
    .iter()
    .any(|key| out.get(key).is_some())
        || ["min_version", "max_version"]
            .iter()
            .any(|key| out["tls"].get(key).is_some())
        || (kind == "vmess"
            && (out["transport"].get("max_early_data").is_some()
                || out["tls"].get("alpn").is_some()))
    {
        return Err(unsupported());
    }
    if matches!(kind, "http" | "naive")
        && out["tls"]["enabled"] == true
        && out["tls"]["server_name"]
            .as_str()
            .is_some_and(|s| Some(s) != out["server"].as_str())
    {
        return Err(unsupported());
    }
    let text = |key: &str| out[key].as_str().unwrap_or("");
    let host = text("server");
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.into()
    };
    let address = format!("{host}:{}", out["server_port"].as_u64().unwrap_or(0));
    let fragment = percent_encode(name);
    let tls = &out["tls"];
    let mut query = Vec::<(String, String)>::new();
    let mut add = |k: &str, v: &str| {
        if !v.is_empty() {
            query.push((k.into(), v.into()));
        }
    };
    if kind == "vmess" {
        if out["transport"]["type"] == "grpc" || tls["reality"]["enabled"] == true {
            return Err(unsupported());
        }
        return Ok(format!("vmess://{}",STANDARD.encode(json!({"v":"2","ps":name,"add":text("server"),"port":out["server_port"].as_u64().unwrap_or(0).to_string(),"id":text("uuid"),"aid":"0","scy":text("security"),"net":out["transport"]["type"].as_str().unwrap_or("tcp"),"type":"none","host":out["transport"]["headers"]["Host"].as_str().or(out["transport"]["host"].as_str()).unwrap_or(""),"path":out["transport"]["path"].as_str().unwrap_or(""),"tls":if tls["enabled"]==true{"tls"}else{""},"sni":tls["server_name"].as_str().unwrap_or("")}).to_string())));
    }
    let identity = match kind {
        "vless" => {
            add("encryption", "none");
            add("flow", text("flow"));
            percent_encode(text("uuid"))
        }
        "shadowsocks" => {
            return Ok(format!(
                "ss://{}@{address}#{fragment}",
                URL_SAFE_NO_PAD.encode(format!("{}:{}", text("method"), text("password")))
            ));
        }
        "trojan" | "hysteria2" | "anytls" => percent_encode(text("password")),
        "tuic" => format!(
            "{}:{}",
            percent_encode(text("uuid")),
            percent_encode(text("password"))
        ),
        "http" | "socks" | "naive" => format!(
            "{}:{}",
            percent_encode(text("username")),
            percent_encode(text("password"))
        ),
        _ => return Err(unsupported()),
    };
    if tls["enabled"] == true {
        add(
            "security",
            if tls["reality"]["enabled"] == true {
                "reality"
            } else {
                "tls"
            },
        );
        add("sni", tls["server_name"].as_str().unwrap_or(""));
        add("fp", tls["utls"]["fingerprint"].as_str().unwrap_or(""));
        add("pbk", tls["reality"]["public_key"].as_str().unwrap_or(""));
        add("sid", tls["reality"]["short_id"].as_str().unwrap_or(""));
        if tls.get("certificate").is_some() {
            return Err(unsupported());
        }
        if let Some(alpn) = tls["alpn"].as_array() {
            add(
                "alpn",
                &alpn
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
    }
    let transport = &out["transport"];
    add("type", transport["type"].as_str().unwrap_or("tcp"));
    add("path", transport["path"].as_str().unwrap_or(""));
    add(
        "host",
        transport["headers"]["Host"]
            .as_str()
            .or(transport["host"].as_str())
            .unwrap_or(""),
    );
    add(
        "serviceName",
        transport["service_name"].as_str().unwrap_or(""),
    );
    if let Some(early) = transport["max_early_data"].as_u64() {
        add("ed", &early.to_string());
        add(
            "eh",
            transport["early_data_header_name"].as_str().unwrap_or(""),
        );
    }
    if kind == "hysteria2" {
        add("obfs", out["obfs"]["type"].as_str().unwrap_or(""));
        add(
            "obfs-password",
            out["obfs"]["password"].as_str().unwrap_or(""),
        );
    }
    if kind == "tuic" {
        add("congestion_control", text("congestion_control"));
        add("udp_relay_mode", text("udp_relay_mode"));
    }
    let scheme = match kind {
        "http" if tls["enabled"] == true => "https",
        "socks" => "socks5",
        "naive" => "naive+https",
        kind => kind,
    };
    let query = query
        .into_iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(&v)))
        .collect::<Vec<_>>()
        .join("&");
    Ok(format!(
        "{scheme}://{identity}@{address}?{query}#{fragment}"
    ))
}

//! Validated, self-contained proxy definitions from subscription imports.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::net::IpAddr;

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct ExternalOutbound(pub Value);

impl std::fmt::Debug for ExternalOutbound {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalOutbound")
            .field("protocol", &self.protocol())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalTransport {
    Tcp,
    Udp,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExternalCapabilities {
    pub required_transport: ExternalTransport,
    pub udp_transport: ExternalTransport,
    pub tcp: bool,
    pub udp: bool,
    pub required_features: Vec<String>,
}

impl ExternalCapabilities {
    pub fn carries(&self, transport: ExternalTransport) -> bool {
        match transport {
            ExternalTransport::Tcp => self.tcp,
            ExternalTransport::Udp => self.udp,
        }
    }
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{0}")]
pub struct ExternalError(pub &'static str);

type Result<T> = std::result::Result<T, ExternalError>;

impl ExternalOutbound {
    pub fn protocol(&self) -> &str {
        self.0["type"].as_str().unwrap_or_default()
    }
    pub fn server(&self) -> &str {
        self.0["server"].as_str().unwrap_or_default()
    }
    pub fn port(&self) -> u16 {
        self.0["server_port"].as_u64().unwrap_or_default() as u16
    }

    pub fn validate(&self) -> Result<()> {
        let object = object(&self.0)?;
        let protocol = text(object, "type", true)?;
        let specific: &[&str] = match protocol {
            "shadowsocks" => &["method", "password", "network", "udp_over_tcp", "multiplex"],
            "vmess" => &[
                "uuid",
                "security",
                "alter_id",
                "global_padding",
                "authenticated_length",
                "network",
                "tls",
                "packet_encoding",
                "multiplex",
                "transport",
            ],
            "vless" => &[
                "uuid",
                "flow",
                "network",
                "tls",
                "packet_encoding",
                "multiplex",
                "transport",
            ],
            "trojan" => &["password", "network", "tls", "multiplex", "transport"],
            "hysteria2" => &["password", "up_mbps", "down_mbps", "obfs", "network", "tls"],
            "tuic" => &[
                "uuid",
                "password",
                "congestion_control",
                "udp_relay_mode",
                "udp_over_stream",
                "zero_rtt_handshake",
                "heartbeat",
                "network",
                "tls",
            ],
            "anytls" => &[
                "password",
                "idle_session_check_interval",
                "idle_session_timeout",
                "min_idle_session",
                "tls",
            ],
            "socks" => &["version", "username", "password", "network", "udp_over_tcp"],
            "http" => &["username", "password", "path", "headers", "tls"],
            "naive" => &["username", "password", "insecure_concurrency", "tls"],
            _ => return Err(ExternalError("unsupported outbound protocol")),
        };
        if object.keys().any(|key| {
            !["type", "server", "server_port"].contains(&key.as_str())
                && !specific.contains(&key.as_str())
        }) {
            return Err(ExternalError("unsupported outbound parameter"));
        }
        valid_host(text(object, "server", true)?)?;
        integer(object, "server_port", 1, 65535, true)?;
        for key in [
            "username",
            "password",
            "flow",
            "path",
            "heartbeat",
            "idle_session_check_interval",
            "idle_session_timeout",
        ] {
            text(object, key, false)?;
        }
        for key in [
            "global_padding",
            "authenticated_length",
            "udp_over_stream",
            "zero_rtt_handshake",
        ] {
            boolean(object, key)?;
        }
        for key in [
            "alter_id",
            "up_mbps",
            "down_mbps",
            "min_idle_session",
            "insecure_concurrency",
        ] {
            integer(object, key, 0, 1_000_000, false)?;
        }
        if let Some(network) = object.get("network") {
            validate_network(network)?;
        }
        if ["vmess", "vless", "tuic"].contains(&protocol) {
            uuid::Uuid::parse_str(text(object, "uuid", true)?)
                .map_err(|_| ExternalError("invalid proxy UUID"))?;
        }
        if ["shadowsocks", "trojan", "hysteria2", "tuic", "anytls"].contains(&protocol) {
            text(object, "password", true)?;
        }
        if protocol == "shadowsocks" {
            validate_ss(object)?;
        }
        enumeration(
            object,
            "security",
            &[
                "auto",
                "none",
                "zero",
                "aes-128-cfb",
                "aes-128-gcm",
                "chacha20-poly1305",
            ],
        )?;
        enumeration(object, "flow", &["", "xtls-rprx-vision"])?;
        enumeration(object, "packet_encoding", &["", "packetaddr", "xudp"])?;
        enumeration(object, "version", &["4", "4a", "5"])?;
        enumeration(object, "congestion_control", &["cubic", "new_reno", "bbr"])?;
        enumeration(object, "udp_relay_mode", &["native", "quic"])?;
        if let Some(tls) = object.get("tls") {
            validate_tls(tls)?;
            if protocol == "naive" {
                object_keys(tls, &["enabled", "server_name", "certificate"])?;
            }
        }
        if ["hysteria2", "tuic", "anytls", "naive"].contains(&protocol)
            && self.0.pointer("/tls/enabled").and_then(Value::as_bool) != Some(true)
        {
            return Err(ExternalError("outbound requires TLS"));
        }
        if let Some(transport) = object.get("transport") {
            validate_transport(transport)?;
        }
        if let Some(mux) = object.get("multiplex") {
            validate_mux(mux)?;
        }
        if let Some(uot) = object.get("udp_over_tcp")
            && !uot.is_boolean()
        {
            let uot = object_keys(uot, &["enabled", "version"])?;
            boolean(uot, "enabled")?;
            integer(uot, "version", 1, 2, false)?;
        }
        if let Some(obfs) = object.get("obfs") {
            let obfs = object_keys(obfs, &["type", "password"])?;
            if text(obfs, "type", true)? != "salamander" {
                return Err(ExternalError("unsupported obfuscation"));
            }
            text(obfs, "password", true)?;
        }
        if let Some(headers) = object.get("headers") {
            validate_headers(headers)?;
        }
        if self.0["flow"] == "xtls-rprx-vision"
            && (object.contains_key("transport")
                || self.0.pointer("/tls/enabled").and_then(Value::as_bool) != Some(true))
        {
            return Err(ExternalError("Vision requires native TLS transport"));
        }
        if self
            .0
            .pointer("/tls/reality/enabled")
            .and_then(Value::as_bool)
            == Some(true)
            && (protocol != "vless"
                || self.0.pointer("/tls/enabled").and_then(Value::as_bool) != Some(true))
        {
            return Err(ExternalError("Reality requires VLESS with TLS"));
        }
        Ok(())
    }

    pub fn capabilities(&self) -> Result<ExternalCapabilities> {
        self.validate()?;
        let protocol = self.protocol();
        let transport = if ["hysteria2", "tuic"].contains(&protocol)
            || self.0.pointer("/transport/type").and_then(Value::as_str) == Some("quic")
        {
            ExternalTransport::Udp
        } else {
            ExternalTransport::Tcp
        };
        let mut tcp = true;
        let mut udp = !["http", "naive"].contains(&protocol)
            && !(protocol == "socks" && self.0["version"].as_str().is_some_and(|v| v != "5"));
        if let Some(network) = self.0.get("network") {
            tcp &= network_contains(network, "tcp");
            udp &= network_contains(network, "udp");
        }
        let uot = self.0["udp_over_tcp"].as_bool() == Some(true)
            || self
                .0
                .pointer("/udp_over_tcp/enabled")
                .and_then(Value::as_bool)
                == Some(true);
        let mux = self
            .0
            .pointer("/multiplex/enabled")
            .and_then(Value::as_bool)
            == Some(true);
        let udp_transport = if ["shadowsocks", "socks"].contains(&protocol) && !uot && !mux {
            ExternalTransport::Udp
        } else {
            transport
        };
        let mut required_features = Vec::new();
        if transport == ExternalTransport::Udp {
            required_features.push("with_quic".into());
        }
        if self.0.pointer("/tls/utls/enabled").and_then(Value::as_bool) == Some(true) {
            required_features.push("with_utls".into());
        }
        if protocol == "naive" {
            required_features.push("with_naive_outbound".into());
        }
        Ok(ExternalCapabilities {
            required_transport: transport,
            udp_transport,
            tcp,
            udp,
            required_features,
        })
    }

    pub fn render(
        &self,
        tag: &str,
        detour: Option<&str>,
        domain_resolver: Option<&str>,
    ) -> Result<Value> {
        self.validate()?;
        if tag.is_empty() || detour == Some(tag) {
            return Err(ExternalError("invalid generated outbound tag"));
        }
        let mut value = self.0.clone();
        value["tag"] = json!(tag);
        if let Some(detour) = detour {
            value["detour"] = json!(detour);
        }
        if self.server().parse::<IpAddr>().is_err()
            && let Some(resolver) = domain_resolver
        {
            value["domain_resolver"] = json!(resolver);
        }
        Ok(value)
    }

    /// Client-facing form of a typed outbound from ordered subscription sources.
    /// Its serde layout is the sing-box outbound shape. The ordered parser fills
    /// a few options with their defaults when a node omits them; those defaults
    /// are left out again here, so the result is what numbered sources accept.
    /// Any other option `validate` does not accept makes the node unusable in
    /// client subscriptions, as it was for numbered sources.
    pub fn from_normalized(outbound: &NormalizedOutbound) -> Result<Self> {
        let mut value = serde_json::to_value(outbound)
            .map_err(|_| ExternalError("outbound is not serializable"))?;
        let defaults: &[(&str, Value)] = match outbound {
            NormalizedOutbound::Hysteria2 { .. } => &[
                ("server_ports", json!([])),
                ("bbr_profile", json!("standard")),
                ("disable_chrome_parrot", json!(false)),
            ],
            NormalizedOutbound::Tuic { .. } => &[("udp_relay_mode", json!(""))],
            _ => &[],
        };
        if let Some(object) = value.as_object_mut() {
            for (key, default) in defaults {
                if object.get(*key) == Some(default) {
                    object.remove(*key);
                }
            }
        }
        let outbound = Self(value);
        outbound.validate()?;
        Ok(outbound)
    }

    pub fn to_outbound(&self, tag: &str, detour: Option<&str>) -> Result<Value> {
        self.render(tag, detour, None)
    }

    /// Authentication, names, and provider-controlled tags never identify a node.
    pub fn identity_value(&self) -> Value {
        json!({"type":self.protocol(),"server":self.server(),"server_port":self.port(),
            "sni":self.0.pointer("/tls/server_name"),"tls":self.0.pointer("/tls/enabled"),
            "transport":{
                "type":self.0.pointer("/transport/type"),
                "host":self.0.pointer("/transport/host").or_else(|| self.0.pointer("/transport/headers/Host")),
                "path":self.0.pointer("/transport/path"),
                "service_name":self.0.pointer("/transport/service_name")
            },"network":self.0.get("network")})
    }
}

fn validate_ss(value: &Map<String, Value>) -> Result<()> {
    let method = text(value, "method", true)?;
    let password = text(value, "password", true)?;
    match method {
        "aes-128-gcm"
        | "aes-192-gcm"
        | "aes-256-gcm"
        | "chacha20-ietf-poly1305"
        | "xchacha20-ietf-poly1305"
        | "none" => Ok(()),
        "2022-blake3-aes-128-gcm" | "2022-blake3-aes-256-gcm" | "2022-blake3-chacha20-poly1305" => {
            let key_size = if method == "2022-blake3-aes-128-gcm" {
                16
            } else {
                32
            };
            for part in password.split(':') {
                if STANDARD
                    .decode(part)
                    .map_or(true, |value| value.len() != key_size)
                {
                    return Err(ExternalError("invalid Shadowsocks 2022 key"));
                }
            }
            Ok(())
        }
        _ => Err(ExternalError("unsupported Shadowsocks cipher")),
    }
}

fn validate_tls(value: &Value) -> Result<()> {
    let value = object_keys(
        value,
        &[
            "enabled",
            "disable_sni",
            "server_name",
            "insecure",
            "alpn",
            "min_version",
            "max_version",
            "cipher_suites",
            "certificate",
            "certificate_public_key_sha256",
            "utls",
            "reality",
        ],
    )?;
    for key in ["enabled", "disable_sni", "insecure"] {
        boolean(value, key)?;
    }
    let sni = text(value, "server_name", false)?;
    if !sni.is_empty() {
        valid_host(sni)?;
    }
    for key in [
        "alpn",
        "cipher_suites",
        "certificate",
        "certificate_public_key_sha256",
    ] {
        if let Some(list) = value.get(key) {
            string_list(list)?;
        }
    }
    for key in ["min_version", "max_version"] {
        enumeration(value, key, &["1.0", "1.1", "1.2", "1.3"])?;
    }
    if let Some(utls) = value.get("utls") {
        let utls = object_keys(utls, &["enabled", "fingerprint"])?;
        boolean(utls, "enabled")?;
        enumeration(
            utls,
            "fingerprint",
            &[
                "chrome_psk",
                "chrome_psk_shuffle",
                "chrome_padding_psk_shuffle",
                "chrome_pq",
                "chrome_pq_psk",
                "chrome",
                "firefox",
                "edge",
                "safari",
                "360",
                "qq",
                "ios",
                "android",
                "random",
                "randomized",
            ],
        )?;
    }
    if let Some(reality) = value.get("reality") {
        let reality = object_keys(reality, &["enabled", "public_key", "short_id"])?;
        boolean(reality, "enabled")?;
        if reality.get("enabled").and_then(Value::as_bool) == Some(true) {
            let key = text(reality, "public_key", true)?;
            if base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(key)
                .map_or(true, |v| v.len() != 32)
            {
                return Err(ExternalError("invalid Reality public key"));
            }
            let short = text(reality, "short_id", false)?;
            if short.len() > 16
                || !short.len().is_multiple_of(2)
                || !short.bytes().all(|c| c.is_ascii_hexdigit())
            {
                return Err(ExternalError("invalid Reality short ID"));
            }
        }
    }
    Ok(())
}

fn validate_transport(value: &Value) -> Result<()> {
    let map = object(value)?;
    let allowed: &[&str] = match text(map, "type", true)? {
        "ws" => &[
            "type",
            "path",
            "headers",
            "max_early_data",
            "early_data_header_name",
        ],
        "http" => &[
            "type",
            "host",
            "path",
            "method",
            "headers",
            "idle_timeout",
            "ping_timeout",
        ],
        "httpupgrade" => &["type", "host", "path", "headers"],
        "grpc" => &[
            "type",
            "service_name",
            "idle_timeout",
            "ping_timeout",
            "permit_without_stream",
        ],
        "quic" => &["type"],
        _ => return Err(ExternalError("unsupported proxy transport")),
    };
    object_keys(value, allowed)?;
    for key in [
        "path",
        "method",
        "early_data_header_name",
        "service_name",
        "idle_timeout",
        "ping_timeout",
    ] {
        text(map, key, false)?;
    }
    if let Some(host) = map.get("host") {
        string_list(host)?;
    }
    if let Some(headers) = map.get("headers") {
        validate_headers(headers)?;
    }
    integer(map, "max_early_data", 0, 65536, false)?;
    boolean(map, "permit_without_stream")?;
    Ok(())
}

fn validate_mux(value: &Value) -> Result<()> {
    let value = object_keys(
        value,
        &[
            "enabled",
            "protocol",
            "max_connections",
            "min_streams",
            "max_streams",
            "padding",
            "brutal",
        ],
    )?;
    boolean(value, "enabled")?;
    boolean(value, "padding")?;
    enumeration(value, "protocol", &["smux", "yamux", "h2mux"])?;
    for key in ["max_connections", "min_streams", "max_streams"] {
        integer(value, key, 0, 65536, false)?;
    }
    if let Some(brutal) = value.get("brutal") {
        let brutal = object_keys(brutal, &["enabled", "up_mbps", "down_mbps"])?;
        boolean(brutal, "enabled")?;
        for key in ["up_mbps", "down_mbps"] {
            integer(brutal, key, 1, 1_000_000, false)?;
        }
    }
    Ok(())
}

fn validate_network(value: &Value) -> Result<()> {
    let valid = match value {
        Value::String(value) => ["tcp", "udp", "tcp,udp", "udp,tcp"].contains(&value.as_str()),
        Value::Array(values) => {
            !values.is_empty()
                && values.len() <= 2
                && values
                    .iter()
                    .all(|value| value.as_str().is_some_and(|s| ["tcp", "udp"].contains(&s)))
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(ExternalError("invalid proxy network"))
    }
}

fn network_contains(value: &Value, network: &str) -> bool {
    match value {
        Value::String(value) => value.split(',').any(|v| v == network),
        Value::Array(values) => values.iter().any(|v| v.as_str() == Some(network)),
        _ => false,
    }
}

fn valid_host(value: &str) -> Result<()> {
    if value.parse::<IpAddr>().is_ok() || apsp_protocol::names::hostname(value) {
        Ok(())
    } else {
        Err(ExternalError("invalid proxy hostname"))
    }
}

fn object(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .ok_or(ExternalError("proxy parameter must be an object"))
}
fn object_keys<'a>(value: &'a Value, keys: &[&str]) -> Result<&'a Map<String, Value>> {
    let value = object(value)?;
    if value.keys().any(|k| !keys.contains(&k.as_str())) {
        return Err(ExternalError("unsupported nested proxy parameter"));
    }
    Ok(value)
}
fn text<'a>(value: &'a Map<String, Value>, key: &str, required: bool) -> Result<&'a str> {
    match value.get(key) {
        None if !required => Ok(""),
        Some(Value::String(value))
            if value.len() <= 65536
                && (!required || !value.is_empty())
                && !value.contains('\0') =>
        {
            Ok(value)
        }
        _ => Err(ExternalError("invalid proxy text parameter")),
    }
}
fn boolean(value: &Map<String, Value>, key: &str) -> Result<()> {
    if value.get(key).is_some_and(|v| !v.is_boolean()) {
        Err(ExternalError("invalid proxy boolean parameter"))
    } else {
        Ok(())
    }
}
fn integer(
    value: &Map<String, Value>,
    key: &str,
    min: u64,
    max: u64,
    required: bool,
) -> Result<()> {
    if (!required && !value.contains_key(key))
        || value
            .get(key)
            .and_then(Value::as_u64)
            .is_some_and(|n| (min..=max).contains(&n))
    {
        Ok(())
    } else {
        Err(ExternalError("invalid proxy integer parameter"))
    }
}
fn enumeration(value: &Map<String, Value>, key: &str, options: &[&str]) -> Result<()> {
    if value
        .get(key)
        .is_some_and(|v| v.as_str().is_none_or(|v| !options.contains(&v)))
    {
        Err(ExternalError("unsupported proxy option"))
    } else {
        Ok(())
    }
}
fn string_list(value: &Value) -> Result<()> {
    let valid = |value: &Value| {
        value
            .as_str()
            .is_some_and(|v| !v.is_empty() && v.len() <= 65536 && !v.contains('\0'))
    };
    if valid(value)
        || value
            .as_array()
            .is_some_and(|v| v.len() <= 128 && v.iter().all(valid))
    {
        Ok(())
    } else {
        Err(ExternalError("invalid proxy string list"))
    }
}
fn validate_headers(value: &Value) -> Result<()> {
    let value = object(value)?;
    if value.len() > 64 {
        return Err(ExternalError("too many proxy headers"));
    }
    for (key, value) in value {
        if key.is_empty()
            || key.len() > 128
            || !key.bytes().all(|v| v.is_ascii_alphanumeric() || v == b'-')
        {
            return Err(ExternalError("invalid proxy header name"));
        }
        string_list(value)?;
        let invalid = |value: &Value| value.as_str().is_some_and(|v| v.contains(['\r', '\n']));
        if invalid(value) || value.as_array().is_some_and(|v| v.iter().any(invalid)) {
            return Err(ExternalError("invalid proxy header value"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unowned_dialers_and_unknown_security_semantics() {
        for key in [
            "detour",
            "domain_resolver",
            "bind_interface",
            "password_file",
            "plugin",
            "route",
        ] {
            let mut value = json!({"type":"http","server":"proxy.example.com","server_port":443});
            value[key] = json!("private-value");
            assert!(ExternalOutbound(value).validate().is_err());
        }
        assert!(ExternalOutbound(json!({"type":"http","server":"proxy.example.com","server_port":443,"tls":{"enabled":true,"certificate_path":"/tmp/secret"}})).validate().is_err());
    }

    #[test]
    fn separates_user_networks_from_lower_transport() {
        let http =
            ExternalOutbound(json!({"type":"http","server":"proxy.example.com","server_port":443}));
        let caps = http.capabilities().unwrap();
        assert!(caps.tcp && !caps.udp);
        let ss = ExternalOutbound(
            json!({"type":"shadowsocks","server":"proxy.example.com","server_port":443,"method":"aes-128-gcm","password":"fixture-password"}),
        );
        assert_eq!(
            ss.capabilities().unwrap().udp_transport,
            ExternalTransport::Udp
        );
        let mut uot = ss.clone();
        uot.0["udp_over_tcp"] = json!(true);
        assert_eq!(
            uot.capabilities().unwrap().udp_transport,
            ExternalTransport::Tcp
        );
    }
}

mod normalized;
pub use normalized::{
    Common, Ech, ExternalProtocol, HysteriaObfs, Multiplex, NormalizedOutbound, Reality, Tls,
    Transport, UdpOverTcp, Utls,
};

use super::{
    MAX_NODES, NodePreview, ParseError, ParseReason, ParseStatus, ParsedNode, display_name,
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use reqwest::Url;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

use apsp_compiler::external::{
    Common, Ech, HysteriaObfs, Multiplex, Reality, Tls, UdpOverTcp, Utls,
};
pub use apsp_compiler::external::{ExternalProtocol, NormalizedOutbound, Transport};

pub(super) fn invalid() -> ParseReason {
    ParseReason::new("invalid_parameter", "节点参数无效或参数类型不正确")
}
pub(super) fn credential() -> ParseReason {
    ParseReason::new("invalid_credential", "节点认证参数无效")
}
pub(super) fn unsupported() -> ParseReason {
    ParseReason::new("unsupported_parameter", "节点包含尚未支持的必要参数")
}
pub(super) fn dependency() -> ParseReason {
    ParseReason::new(
        "unsupported_dependency",
        "节点依赖原配置、宿主资源或外部文件，无法独立导入",
    )
}

pub(super) fn host(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > 253
        || value.chars().any(|c| c.is_whitespace() || c.is_control())
        || value.contains(['/', '?', '#', '@', '%', '\\'])
    {
        return None;
    }
    let bare = value
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(value);
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        return Some(ip.to_string());
    }
    if value.contains([':', '[', ']']) {
        return None;
    }
    let url = Url::parse(&format!("https://{value}/")).ok()?;
    let domain = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty()
        || domain.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
    {
        return None;
    }
    Some(domain)
}

pub(super) fn port(value: &Value) -> Option<u16> {
    value
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0)
}

fn keys(value: &Map<String, Value>, allowed: &[&str]) -> Result<(), ParseReason> {
    if value.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(unsupported());
    }
    Ok(())
}

pub(super) fn string(value: &Map<String, Value>, key: &str) -> Result<String, ParseReason> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(invalid)
}

fn optional_string(value: &Map<String, Value>, key: &str) -> Result<Option<String>, ParseReason> {
    value
        .get(key)
        .map(|value| value.as_str().map(str::to_owned).ok_or_else(invalid))
        .transpose()
}

pub(super) fn boolean(
    value: &Map<String, Value>,
    key: &str,
    default: bool,
) -> Result<bool, ParseReason> {
    value
        .get(key)
        .map(|value| value.as_bool().ok_or_else(invalid))
        .unwrap_or(Ok(default))
}

fn unsigned(
    value: &Map<String, Value>,
    key: &str,
    default: u64,
    maximum: u64,
) -> Result<u64, ParseReason> {
    let number = value
        .get(key)
        .map(|value| value.as_u64().ok_or_else(invalid))
        .unwrap_or(Ok(default))?;
    if number > maximum {
        return Err(invalid());
    }
    Ok(number)
}

pub(super) fn list(value: &Value) -> Result<Vec<String>, ParseReason> {
    match value {
        Value::String(value) => Ok(vec![value.clone()]),
        Value::Array(values) if values.len() <= 256 => values
            .iter()
            .map(|value| value.as_str().map(str::to_owned).ok_or_else(invalid))
            .collect(),
        _ => Err(invalid()),
    }
}

fn strings(value: &Map<String, Value>, key: &str) -> Result<Vec<String>, ParseReason> {
    value.get(key).map(list).unwrap_or_else(|| Ok(Vec::new()))
}

fn choice(
    value: &Map<String, Value>,
    key: &str,
    default: &str,
    allowed: &[&str],
) -> Result<String, ParseReason> {
    let value = optional_string(value, key)?.unwrap_or_else(|| default.into());
    if !allowed.contains(&value.as_str()) {
        return Err(invalid());
    }
    Ok(value)
}

pub(super) fn duration(value: &str) -> Result<String, ParseReason> {
    if value.is_empty()
        || value.len() > 32
        || !value
            .bytes()
            .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'h' | b'm' | b's' | b'u' | b'n'))
    {
        return Err(invalid());
    }
    let unit = ["ms", "us", "ns", "s", "m", "h"]
        .into_iter()
        .find(|unit| value.ends_with(unit))
        .ok_or_else(invalid)?;
    let number = value[..value.len() - unit.len()]
        .parse::<f64>()
        .map_err(|_| invalid())?;
    let multiplier = match unit {
        "h" => 3_600_000_000_000.0,
        "m" => 60_000_000_000.0,
        "s" => 1_000_000_000.0,
        "ms" => 1_000_000.0,
        "us" => 1_000.0,
        "ns" => 1.0,
        _ => return Err(invalid()),
    };
    let nanoseconds = number * multiplier;
    if !nanoseconds.is_finite()
        || nanoseconds <= 0.0
        || nanoseconds > 86_400_000_000_000.0
        || nanoseconds.fract() != 0.0
    {
        return Err(invalid());
    }
    let nanoseconds = nanoseconds as u64;
    for (divisor, unit) in [
        (1_000_000_000, "s"),
        (1_000_000, "ms"),
        (1_000, "us"),
        (1, "ns"),
    ] {
        if nanoseconds.is_multiple_of(divisor) {
            return Ok(format!("{}{unit}", nanoseconds / divisor));
        }
    }
    Err(invalid())
}

fn optional_duration(value: &Map<String, Value>, key: &str) -> Result<Option<String>, ParseReason> {
    optional_string(value, key)?
        .as_deref()
        .map(duration)
        .transpose()
}

fn headers(value: Option<&Value>) -> Result<BTreeMap<String, Vec<String>>, ParseReason> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let value = value.as_object().ok_or_else(invalid)?;
    if value.len() > 64 {
        return Err(invalid());
    }
    let mut result = BTreeMap::new();
    for (key, value) in value {
        if key.is_empty()
            || key.len() > 128
            || !key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
        {
            return Err(invalid());
        }
        let values = list(value)?;
        if values.is_empty()
            || values
                .iter()
                .any(|value| value.contains(['\r', '\n', '\0']))
        {
            return Err(invalid());
        }
        if result.insert(key.to_ascii_lowercase(), values).is_some() {
            return Err(invalid());
        }
    }
    Ok(result)
}

fn tls(value: Option<&Value>) -> Result<Option<Tls>, ParseReason> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.as_object().ok_or_else(invalid)?;
    if value.keys().any(|key| {
        [
            "certificate_path",
            "client_certificate_path",
            "client_key_path",
            "engine",
            "spoof",
            "spoof_method",
            "kernel_tx",
            "kernel_rx",
        ]
        .contains(&key.as_str())
    }) {
        return Err(dependency());
    }
    keys(
        value,
        &[
            "enabled",
            "server_name",
            "disable_sni",
            "insecure",
            "alpn",
            "min_version",
            "max_version",
            "cipher_suites",
            "curve_preferences",
            "certificate",
            "certificate_public_key_sha256",
            "client_certificate",
            "client_key",
            "utls",
            "reality",
            "ech",
        ],
    )?;
    let enabled = boolean(value, "enabled", false)?;
    let server_name = optional_string(value, "server_name")?
        .map(|name| host(&name).ok_or_else(invalid))
        .transpose()?;
    let alpn = strings(value, "alpn")?;
    if alpn.iter().any(|protocol| {
        protocol.is_empty() || protocol.len() > 255 || protocol.chars().any(char::is_control)
    }) {
        return Err(invalid());
    }
    let min_version = optional_string(value, "min_version")?;
    let max_version = optional_string(value, "max_version")?;
    let versions = ["1.0", "1.1", "1.2", "1.3"];
    if min_version
        .iter()
        .chain(max_version.iter())
        .any(|version| !versions.contains(&version.as_str()))
        || min_version
            .as_ref()
            .zip(max_version.as_ref())
            .is_some_and(|(min, max)| min > max)
    {
        return Err(invalid());
    }
    let utls = value
        .get("utls")
        .map(|value| -> Result<Utls, ParseReason> {
            let value = value.as_object().ok_or_else(invalid)?;
            keys(value, &["enabled", "fingerprint"])?;
            Ok(Utls {
                enabled: boolean(value, "enabled", false)?,
                fingerprint: choice(
                    value,
                    "fingerprint",
                    "chrome",
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
                )?,
            })
        })
        .transpose()?;
    let reality = value
        .get("reality")
        .map(|value| -> Result<Reality, ParseReason> {
            let value = value.as_object().ok_or_else(invalid)?;
            keys(value, &["enabled", "public_key", "short_id"])?;
            let public_key = string(value, "public_key")?;
            let short_id = optional_string(value, "short_id")?.unwrap_or_default();
            if URL_SAFE_NO_PAD
                .decode(&public_key)
                .ok()
                .is_none_or(|bytes| bytes.len() != 32)
                || short_id.len() > 16
                || short_id.len() % 2 != 0
                || !short_id.bytes().all(|c| c.is_ascii_hexdigit())
            {
                return Err(credential());
            }
            Ok(Reality {
                enabled: boolean(value, "enabled", false)?,
                public_key,
                short_id: short_id.to_ascii_lowercase(),
            })
        })
        .transpose()?;
    let ech = value
        .get("ech")
        .map(|value| -> Result<Ech, ParseReason> {
            let value = value.as_object().ok_or_else(invalid)?;
            if value.contains_key("config_path") || value.contains_key("query_server_name") {
                return Err(dependency());
            }
            keys(value, &["enabled", "config"])?;
            // ECH acceptance additionally depends on the signed runtime's TLS engine and config support.
            Err(ParseReason::new(
                "unsupported_runtime_capability",
                "节点的 ECH 配置需要独立确认运行时能力",
            ))
        })
        .transpose()?;
    if !enabled
        && (reality.as_ref().is_some_and(|r| r.enabled)
            || utls.as_ref().is_some_and(|u| u.enabled)
            || ech.as_ref().is_some_and(|e| e.enabled))
    {
        return Err(invalid());
    }
    if reality.as_ref().is_some_and(|reality| reality.enabled)
        && utls.as_ref().is_none_or(|utls| !utls.enabled)
    {
        return Err(ParseReason::new(
            "invalid_parameter",
            "Reality 节点需要明确启用 uTLS 指纹",
        ));
    }
    let certificate_public_key_sha256 = strings(value, "certificate_public_key_sha256")?;
    if certificate_public_key_sha256.iter().any(|pin| {
        STANDARD
            .decode(pin)
            .ok()
            .is_none_or(|bytes| bytes.len() != 32)
    }) {
        return Err(invalid());
    }
    let client_certificate = strings(value, "client_certificate")?;
    let client_key = strings(value, "client_key")?;
    if client_certificate.is_empty() != client_key.is_empty() {
        return Err(credential());
    }
    let certificate = strings(value, "certificate")?;
    for certificates in [&certificate, &client_certificate] {
        if !certificates.is_empty() {
            let pem = certificates.join("\n");
            let parsed = CertificateDer::pem_slice_iter(pem.as_bytes())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| credential())?;
            if parsed.is_empty() {
                return Err(credential());
            }
        }
    }
    if !client_key.is_empty()
        && PrivateKeyDer::from_pem_slice(client_key.join("\n").as_bytes()).is_err()
    {
        return Err(credential());
    }
    let curve_preferences = strings(value, "curve_preferences")?;
    if curve_preferences.iter().any(|curve| {
        !["P256", "P384", "P521", "X25519", "X25519MLKEM768"].contains(&curve.as_str())
    }) {
        return Err(invalid());
    }
    let cipher_suites = strings(value, "cipher_suites")?;
    if !cipher_suites.is_empty() {
        return Err(ParseReason::new(
            "unsupported_parameter",
            "节点的指定 TLS 密码套件需要独立确认等价支持",
        ));
    }
    Ok(Some(Tls {
        enabled,
        server_name,
        disable_sni: value
            .get("disable_sni")
            .map(|_| boolean(value, "disable_sni", false))
            .transpose()?,
        insecure: value
            .get("insecure")
            .map(|_| boolean(value, "insecure", false))
            .transpose()?,
        alpn,
        min_version,
        max_version,
        cipher_suites,
        curve_preferences,
        certificate,
        certificate_public_key_sha256,
        client_certificate,
        client_key,
        utls,
        reality,
        ech,
    }))
}

fn transport(value: Option<&Value>) -> Result<Option<Transport>, ParseReason> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.as_object().ok_or_else(invalid)?;
    let kind = string(value, "type")?;
    let path = || optional_string(value, "path").map(|value| value.unwrap_or_default());
    let result = match kind.as_str() {
        "ws" => {
            keys(
                value,
                &[
                    "type",
                    "path",
                    "headers",
                    "max_early_data",
                    "early_data_header_name",
                ],
            )?;
            Transport::Ws {
                path: path()?,
                headers: headers(value.get("headers"))?,
                max_early_data: unsigned(value, "max_early_data", 0, 65_536)? as u32,
                early_data_header_name: optional_string(value, "early_data_header_name")?
                    .unwrap_or_default(),
            }
        }
        "http" => {
            keys(
                value,
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
            let host = strings(value, "host")?
                .iter()
                .map(|value| host(value).ok_or_else(invalid))
                .collect::<Result<Vec<_>, _>>()?;
            Transport::Http {
                host,
                path: path()?,
                method: choice(
                    value,
                    "method",
                    "GET",
                    &["GET", "POST", "PUT", "PATCH", "HEAD", "OPTIONS", "DELETE"],
                )?,
                headers: headers(value.get("headers"))?,
                idle_timeout: optional_duration(value, "idle_timeout")?,
                ping_timeout: optional_duration(value, "ping_timeout")?,
            }
        }
        "grpc" => {
            keys(
                value,
                &[
                    "type",
                    "service_name",
                    "idle_timeout",
                    "ping_timeout",
                    "permit_without_stream",
                ],
            )?;
            Transport::Grpc {
                service_name: optional_string(value, "service_name")?.unwrap_or_default(),
                idle_timeout: optional_duration(value, "idle_timeout")?,
                ping_timeout: optional_duration(value, "ping_timeout")?,
                permit_without_stream: boolean(value, "permit_without_stream", false)?,
            }
        }
        "httpupgrade" => {
            keys(value, &["type", "host", "path", "headers"])?;
            let host = optional_string(value, "host")?
                .map(|value| host(&value).ok_or_else(invalid))
                .transpose()?
                .unwrap_or_default();
            Transport::Httpupgrade {
                host,
                path: path()?,
                headers: headers(value.get("headers"))?,
            }
        }
        "quic" => {
            keys(value, &["type"])?;
            Transport::Quic {}
        }
        _ => {
            return Err(ParseReason::new(
                "unsupported_transport",
                "节点的传输方式尚未支持",
            ));
        }
    };
    Ok(Some(result))
}

fn multiplex(value: Option<&Value>) -> Result<Option<Multiplex>, ParseReason> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.as_object().ok_or_else(invalid)?;
    if value.contains_key("brutal") {
        return Err(dependency());
    }
    keys(
        value,
        &[
            "enabled",
            "protocol",
            "max_connections",
            "min_streams",
            "max_streams",
            "padding",
        ],
    )?;
    let max_connections = unsigned(value, "max_connections", 0, 1024)? as u16;
    let min_streams = unsigned(value, "min_streams", 0, 4096)? as u16;
    let max_streams = unsigned(value, "max_streams", 0, 4096)? as u16;
    if min_streams != 0 && max_streams != 0 && min_streams > max_streams {
        return Err(invalid());
    }
    Ok(Some(Multiplex {
        enabled: boolean(value, "enabled", false)?,
        protocol: choice(value, "protocol", "h2mux", &["h2mux", "smux", "yamux"])?,
        max_connections,
        min_streams,
        max_streams,
        padding: boolean(value, "padding", false)?,
    }))
}

fn uot(value: Option<&Value>) -> Result<Option<UdpOverTcp>, ParseReason> {
    let Some(value) = value else {
        return Ok(None);
    };
    if let Some(enabled) = value.as_bool() {
        return Ok(Some(UdpOverTcp {
            enabled,
            version: 2,
        }));
    }
    let value = value.as_object().ok_or_else(invalid)?;
    keys(value, &["enabled", "version"])?;
    let version = unsigned(value, "version", 2, 2)? as u8;
    if version == 0 {
        return Err(invalid());
    }
    Ok(Some(UdpOverTcp {
        enabled: boolean(value, "enabled", false)?,
        version,
    }))
}

fn common(value: &Map<String, Value>, protocol: &str) -> Result<Common, ParseReason> {
    let server = host(&string(value, "server")?).ok_or_else(invalid)?;
    let server_port = value
        .get("server_port")
        .and_then(port)
        .ok_or_else(invalid)?;
    let mut network = value
        .get("network")
        .map(list)
        .unwrap_or_else(|| Ok(vec!["tcp".into(), "udp".into()]))?;
    if network.is_empty()
        || network
            .iter()
            .any(|network| !["tcp", "udp"].contains(&network.as_str()))
    {
        return Err(invalid());
    }
    network.sort();
    network.dedup();
    if ["http", "anytls"].contains(&protocol) {
        network.clear();
    }
    let tls = tls(value.get("tls"))?;
    if ["hysteria2", "tuic", "anytls"].contains(&protocol)
        && tls.as_ref().is_none_or(|tls| !tls.enabled)
    {
        return Err(invalid());
    }
    if ["hysteria2", "tuic"].contains(&protocol)
        && tls.as_ref().is_some_and(|tls| {
            tls.utls.as_ref().is_some_and(|utls| utls.enabled)
                || tls.reality.as_ref().is_some_and(|reality| reality.enabled)
        })
    {
        return Err(ParseReason::new(
            "unsupported_transport",
            "QUIC 节点无法使用该 TLS 握手方式",
        ));
    }
    Ok(Common {
        server,
        server_port,
        network,
        tls,
        transport: transport(value.get("transport"))?,
        multiplex: multiplex(value.get("multiplex"))?,
        udp_over_tcp: uot(value.get("udp_over_tcp"))?,
        connect_timeout: optional_duration(value, "connect_timeout")?,
        tcp_fast_open: value
            .get("tcp_fast_open")
            .map(|_| boolean(value, "tcp_fast_open", false))
            .transpose()?,
        tcp_multi_path: value
            .get("tcp_multi_path")
            .map(|_| boolean(value, "tcp_multi_path", false))
            .transpose()?,
        udp_fragment: value
            .get("udp_fragment")
            .map(|_| boolean(value, "udp_fragment", false))
            .transpose()?,
    })
}

fn uuid(value: &Map<String, Value>) -> Result<String, ParseReason> {
    let value = string(value, "uuid").map_err(|_| credential())?;
    uuid::Uuid::parse_str(&value)
        .map(|value| value.hyphenated().to_string())
        .map_err(|_| credential())
}

fn password(value: &Map<String, Value>) -> Result<String, ParseReason> {
    let value = string(value, "password").map_err(|_| credential())?;
    if value.is_empty() || value.contains('\0') {
        return Err(credential());
    }
    Ok(value)
}

pub(super) fn normalize(value: &Value) -> Result<NormalizedOutbound, ParseReason> {
    let value = value.as_object().ok_or_else(invalid)?;
    if value.keys().any(|key| {
        [
            "detour",
            "bind_interface",
            "inet4_bind_address",
            "inet6_bind_address",
            "bind_address_no_port",
            "protect_path",
            "routing_mark",
            "reuse_addr",
            "netns",
            "domain_resolver",
            "domain_strategy",
            "realm",
        ]
        .contains(&key.as_str())
    }) {
        return Err(dependency());
    }
    let protocol = string(value, "type")?;
    let extra: &[&str] = match protocol.as_str() {
        "shadowsocks" => &[
            "method",
            "password",
            "plugin",
            "plugin_opts",
            "network",
            "multiplex",
            "udp_over_tcp",
        ],
        "vmess" => &[
            "uuid",
            "security",
            "alter_id",
            "global_padding",
            "authenticated_length",
            "packet_encoding",
            "network",
            "tls",
            "multiplex",
            "transport",
        ],
        "trojan" => &["password", "network", "tls", "multiplex", "transport"],
        "vless" => &[
            "uuid",
            "flow",
            "packet_encoding",
            "network",
            "tls",
            "multiplex",
            "transport",
        ],
        "hysteria2" => &[
            "password",
            "server_ports",
            "hop_interval",
            "hop_interval_max",
            "up_mbps",
            "down_mbps",
            "obfs",
            "network",
            "tls",
            "bbr_profile",
            "disable_chrome_parrot",
        ],
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
            "client_metadata",
            "tls",
        ],
        "socks" => &["version", "username", "password", "network", "udp_over_tcp"],
        "http" => &["username", "password", "tls", "path", "headers"],
        "naive" => {
            return Err(ParseReason::new(
                "unsupported_runtime_capability",
                "节点需要尚未确认的平台运行时能力",
            ));
        }
        _ => return Err(ParseReason::new("unsupported_protocol", "节点协议尚未支持")),
    };
    let mut allowed = vec![
        "type",
        "tag",
        "server",
        "server_port",
        "connect_timeout",
        "tcp_fast_open",
        "tcp_multi_path",
        "udp_fragment",
    ];
    allowed.extend_from_slice(extra);
    keys(value, &allowed)?;
    let common = common(value, &protocol)?;
    let result = match protocol.as_str() {
        "shadowsocks" => {
            let method = choice(
                value,
                "method",
                "",
                &[
                    "none",
                    "aes-128-gcm",
                    "aes-192-gcm",
                    "aes-256-gcm",
                    "chacha20-ietf-poly1305",
                    "xchacha20-ietf-poly1305",
                    "2022-blake3-aes-128-gcm",
                    "2022-blake3-aes-256-gcm",
                    "2022-blake3-chacha20-poly1305",
                    "aes-128-ctr",
                    "aes-192-ctr",
                    "aes-256-ctr",
                    "aes-128-cfb",
                    "aes-192-cfb",
                    "aes-256-cfb",
                    "rc4-md5",
                    "chacha20-ietf",
                    "xchacha20",
                ],
            )?;
            let password = if method == "none" {
                optional_string(value, "password")?.unwrap_or_default()
            } else {
                password(value)?
            };
            if method.starts_with("2022-") {
                let size = if method == "2022-blake3-aes-128-gcm" {
                    16
                } else {
                    32
                };
                if password.split(':').any(|key| {
                    STANDARD
                        .decode(key)
                        .ok()
                        .is_none_or(|bytes| bytes.len() != size)
                }) {
                    return Err(credential());
                }
                if method == "2022-blake3-chacha20-poly1305" && password.contains(':') {
                    return Err(credential());
                }
            }
            let plugin = optional_string(value, "plugin")?;
            let mut plugin_opts = optional_string(value, "plugin_opts")?;
            if plugin_opts.is_some() && plugin.is_none() {
                return Err(invalid());
            }
            if let Some(plugin) = plugin.as_deref() {
                validate_plugin(plugin, plugin_opts.as_deref().unwrap_or(""))?;
                let mut fields = plugin_fields(plugin_opts.as_deref().unwrap_or(""))?;
                if plugin == "obfs-local" {
                    fields.entry("obfs".into()).or_insert_with(|| "http".into());
                    fields.entry("obfs-host".into()).or_default();
                } else if plugin == "v2ray-plugin" {
                    for (key, value) in [
                        ("mode", "websocket"),
                        ("host", "cloudfront.com"),
                        ("path", "/"),
                        ("mux", "1"),
                    ] {
                        fields.entry(key.into()).or_insert_with(|| value.into());
                    }
                    if fields.contains_key("tls") {
                        fields.insert("tls".into(), String::new());
                    }
                }
                plugin_opts = Some(
                    fields
                        .into_iter()
                        .map(|(key, value)| {
                            if value.is_empty() {
                                escape_plugin(&key)
                            } else {
                                format!("{}={}", escape_plugin(&key), escape_plugin(&value))
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(";"),
                );
            }
            NormalizedOutbound::Shadowsocks {
                common,
                method,
                password,
                plugin,
                plugin_opts,
            }
        }
        "vmess" => NormalizedOutbound::Vmess {
            common,
            uuid: uuid(value)?,
            security: choice(
                value,
                "security",
                "auto",
                &[
                    "auto",
                    "none",
                    "zero",
                    "aes-128-cfb",
                    "aes-128-gcm",
                    "chacha20-poly1305",
                ],
            )?,
            alter_id: unsigned(value, "alter_id", 0, 65535)? as u16,
            global_padding: boolean(value, "global_padding", false)?,
            authenticated_length: boolean(value, "authenticated_length", false)?,
            packet_encoding: optional_string(value, "packet_encoding")?
                .map(|value| {
                    if ["", "packetaddr", "xudp"].contains(&value.as_str()) {
                        Ok(value)
                    } else {
                        Err(invalid())
                    }
                })
                .transpose()?,
        },
        "trojan" => NormalizedOutbound::Trojan {
            common,
            password: password(value)?,
        },
        "vless" => {
            let flow = choice(value, "flow", "", &["", "xtls-rprx-vision"])?;
            if !flow.is_empty()
                && (common.tls.as_ref().is_none_or(|tls| !tls.enabled)
                    || common.transport.is_some())
            {
                return Err(invalid());
            }
            NormalizedOutbound::Vless {
                common,
                uuid: uuid(value)?,
                flow,
                packet_encoding: choice(
                    value,
                    "packet_encoding",
                    "xudp",
                    &["", "packetaddr", "xudp"],
                )?,
            }
        }
        "hysteria2" => {
            let server_ports = strings(value, "server_ports")?;
            if server_ports.iter().any(|ports| !valid_port_range(ports)) {
                return Err(invalid());
            }
            let obfs = value.get("obfs").map(obfs).transpose()?;
            NormalizedOutbound::Hysteria2 {
                common,
                password: password(value)?,
                server_ports,
                hop_interval: optional_duration(value, "hop_interval")?,
                hop_interval_max: optional_duration(value, "hop_interval_max")?,
                up_mbps: positive_mbps(value, "up_mbps")?,
                down_mbps: positive_mbps(value, "down_mbps")?,
                obfs,
                bbr_profile: choice(
                    value,
                    "bbr_profile",
                    "standard",
                    &["standard", "conservative", "aggressive"],
                )?,
                disable_chrome_parrot: boolean(value, "disable_chrome_parrot", false)?,
            }
        }
        "tuic" => {
            let udp_relay_mode = choice(value, "udp_relay_mode", "", &["", "native", "quic"])?;
            let udp_over_stream = boolean(value, "udp_over_stream", false)?;
            if udp_over_stream && !udp_relay_mode.is_empty() {
                return Err(invalid());
            }
            NormalizedOutbound::Tuic {
                common,
                uuid: uuid(value)?,
                password: password(value)?,
                congestion_control: choice(
                    value,
                    "congestion_control",
                    "cubic",
                    &["cubic", "new_reno", "bbr"],
                )?,
                udp_relay_mode,
                udp_over_stream,
                zero_rtt_handshake: boolean(value, "zero_rtt_handshake", false)?,
                heartbeat: optional_duration(value, "heartbeat")?.unwrap_or_else(|| "10s".into()),
            }
        }
        "anytls" => NormalizedOutbound::Anytls {
            common,
            password: password(value)?,
            idle_session_check_interval: optional_duration(value, "idle_session_check_interval")?
                .unwrap_or_else(|| "30s".into()),
            idle_session_timeout: optional_duration(value, "idle_session_timeout")?
                .unwrap_or_else(|| "30s".into()),
            min_idle_session: unsigned(value, "min_idle_session", 0, 1024)? as u16,
            client_metadata: optional_string(value, "client_metadata")?,
        },
        "socks" => {
            let version = choice(value, "version", "5", &["4", "4a", "5"])?;
            let username = optional_string(value, "username")?;
            let password = optional_string(value, "password")?;
            if version == "5" {
                validate_optional_auth(&username, &password)?;
            } else if password
                .as_ref()
                .is_some_and(|password| !password.is_empty())
                || common.udp_over_tcp.as_ref().is_some_and(|uot| uot.enabled)
            {
                return Err(credential());
            }
            let mut common = common;
            if version != "5" {
                if value.contains_key("network")
                    && common.network.iter().any(|network| network == "udp")
                {
                    return Err(invalid());
                }
                common.network = vec!["tcp".into()];
            }
            NormalizedOutbound::Socks {
                common,
                version,
                username,
                password,
            }
        }
        "http" => {
            let username = optional_string(value, "username")?;
            let password = optional_string(value, "password")?;
            validate_optional_auth(&username, &password)?;
            let path = optional_string(value, "path")?.unwrap_or_default();
            if path.contains(['\r', '\n', '\0']) {
                return Err(invalid());
            }
            NormalizedOutbound::Http {
                common,
                username,
                password,
                path,
                headers: headers(value.get("headers"))?,
            }
        }
        _ => return Err(unsupported()),
    };
    Ok(result)
}

fn validate_optional_auth(
    username: &Option<String>,
    password: &Option<String>,
) -> Result<(), ParseReason> {
    if password.is_some() && username.as_ref().is_none_or(|username| username.is_empty())
        || username
            .as_ref()
            .is_some_and(|username| username.contains('\0'))
        || password
            .as_ref()
            .is_some_and(|password| password.contains('\0'))
    {
        return Err(credential());
    }
    Ok(())
}

fn positive_mbps(value: &Map<String, Value>, key: &str) -> Result<Option<u32>, ParseReason> {
    value
        .get(key)
        .map(|value| {
            value
                .as_u64()
                .filter(|value| *value <= 1_000_000)
                .map(|value| if value == 0 { None } else { Some(value as u32) })
                .ok_or_else(invalid)
        })
        .transpose()
        .map(Option::flatten)
}

fn obfs(value: &Value) -> Result<HysteriaObfs, ParseReason> {
    let value = value.as_object().ok_or_else(invalid)?;
    match string(value, "type")?.as_str() {
        "salamander" => {
            keys(value, &["type", "password"])?;
            Ok(HysteriaObfs::Salamander {
                password: password(value)?,
            })
        }
        "gecko" => {
            keys(
                value,
                &["type", "password", "min_packet_size", "max_packet_size"],
            )?;
            let min_packet_size = unsigned(value, "min_packet_size", 0, 65535)? as u16;
            let max_packet_size = unsigned(value, "max_packet_size", 0, 65535)? as u16;
            if max_packet_size != 0 && min_packet_size > max_packet_size {
                return Err(invalid());
            }
            Ok(HysteriaObfs::Gecko {
                password: password(value)?,
                min_packet_size,
                max_packet_size,
            })
        }
        _ => Err(unsupported()),
    }
}

pub(super) fn valid_port_range(value: &str) -> bool {
    value.split(',').all(|range| {
        let parts = range.split(':').collect::<Vec<_>>();
        match parts.as_slice() {
            [port] => port.parse::<u16>().is_ok_and(|port| port > 0),
            [min, max] => min
                .parse::<u16>()
                .ok()
                .zip(max.parse::<u16>().ok())
                .is_some_and(|(min, max)| min > 0 && min <= max),
            _ => false,
        }
    })
}

pub(super) fn plugin_fields(options: &str) -> Result<BTreeMap<String, String>, ParseReason> {
    let mut fields = BTreeMap::new();
    let mut token = String::new();
    let mut escaped = false;
    let mut tokens = Vec::new();
    for c in options.chars() {
        if escaped {
            token.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == ';' {
            tokens.push(std::mem::take(&mut token));
        } else {
            token.push(c);
        }
    }
    if escaped {
        return Err(invalid());
    }
    tokens.push(token);
    for token in tokens.into_iter().filter(|token| !token.is_empty()) {
        let (key, value) = token.split_once('=').unwrap_or((&token, ""));
        if fields.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(invalid());
        }
    }
    Ok(fields)
}

fn escape_plugin(value: &str) -> String {
    let mut result = String::new();
    for c in value.chars() {
        if matches!(c, '\\' | ';' | '=' | ':') {
            result.push('\\');
        }
        result.push(c);
    }
    result
}

fn validate_plugin(plugin: &str, options: &str) -> Result<(), ParseReason> {
    let fields = plugin_fields(options)?;
    match plugin {
        "obfs-local" => {
            if fields
                .keys()
                .any(|key| !["obfs", "obfs-host"].contains(&key.as_str()))
            {
                return Err(unsupported());
            }
            if fields
                .get("obfs")
                .is_some_and(|mode| !["http", "tls"].contains(&mode.as_str()))
            {
                return Err(invalid());
            }
        }
        "v2ray-plugin" => {
            if fields.contains_key("cert") {
                return Err(dependency());
            }
            if fields.contains_key("certRaw") {
                return Err(ParseReason::new(
                    "unsupported_parameter",
                    "代理插件的内嵌证书需要独立确认等价支持",
                ));
            }
            if fields.keys().any(|key| {
                !["mode", "tls", "host", "path", "mux", "certRaw"].contains(&key.as_str())
            }) {
                return Err(unsupported());
            }
            if fields
                .get("mode")
                .is_some_and(|mode| !["websocket", "quic"].contains(&mode.as_str()))
                || fields
                    .get("mux")
                    .is_some_and(|mux| mux.parse::<u16>().is_err())
            {
                return Err(invalid());
            }
        }
        _ => {
            return Err(ParseReason::new(
                "unsupported_dependency",
                "节点需要尚未支持的代理插件",
            ));
        }
    }
    Ok(())
}

pub(super) fn node(value: Value, ordinal: usize, name: Option<&str>) -> ParsedNode {
    let mut preview = NodePreview {
        ordinal,
        name: display_name(name, ordinal),
        protocol: value
            .get("type")
            .and_then(Value::as_str)
            .and_then(|kind| match kind {
                "shadowsocks" => Some(ExternalProtocol::Shadowsocks),
                "vmess" => Some(ExternalProtocol::Vmess),
                "trojan" => Some(ExternalProtocol::Trojan),
                "vless" => Some(ExternalProtocol::Vless),
                "hysteria2" => Some(ExternalProtocol::Hysteria2),
                "tuic" => Some(ExternalProtocol::Tuic),
                "anytls" => Some(ExternalProtocol::Anytls),
                "socks" => Some(ExternalProtocol::Socks),
                "http" => Some(ExternalProtocol::Http),
                _ => None,
            }),
        server: value.get("server").and_then(Value::as_str).and_then(host),
        server_port: value.get("server_port").and_then(port),
        sni: value
            .get("tls")
            .and_then(|tls| tls.get("server_name"))
            .and_then(Value::as_str)
            .and_then(host),
        transport: value
            .get("transport")
            .and_then(|transport| transport.get("type"))
            .and_then(Value::as_str)
            .filter(|kind| ["ws", "http", "grpc", "httpupgrade", "quic"].contains(kind))
            .map(str::to_owned),
        parse_status: ParseStatus::Unsupported,
        unsupported_reasons: Vec::new(),
    };
    preview.name = super::redact_auth_name(&preview.name, &value, ordinal);
    match normalize(&value) {
        Ok(outbound) => {
            preview.parse_status = ParseStatus::Supported;
            preview.server = Some(outbound.common().server.clone());
            preview.server_port = Some(outbound.common().server_port);
            preview.sni = outbound
                .common()
                .tls
                .as_ref()
                .and_then(|tls| tls.server_name.clone());
            preview.transport = Some(
                outbound
                    .common()
                    .transport
                    .as_ref()
                    .map(Transport::kind)
                    .unwrap_or("tcp")
                    .into(),
            );
            let (content_digest, identity_fingerprint) = super::identity::digests(&outbound);
            ParsedNode {
                preview,
                outbound: Some(outbound),
                content_digest: Some(content_digest),
                identity_fingerprint: Some(identity_fingerprint),
                provider_metadata_id: None,
            }
        }
        Err(reason) => {
            preview.unsupported_reasons.push(reason);
            rejected(preview)
        }
    }
}

/// Applies the provider id taken before conversion. Only supported nodes use
/// it for identity; an invalid id makes the node unsupported.
pub(super) fn attach_provider(
    mut parsed: ParsedNode,
    provider: Result<Option<String>, ParseReason>,
) -> ParsedNode {
    match provider {
        Ok(provider) => {
            if parsed.outbound.is_some() {
                parsed.provider_metadata_id = provider;
            }
        }
        Err(reason) => {
            parsed.preview.parse_status = ParseStatus::Unsupported;
            parsed.preview.unsupported_reasons.push(reason);
            parsed.outbound = None;
            parsed.content_digest = None;
            parsed.identity_fingerprint = None;
        }
    }
    parsed
}

pub(super) fn rejected(preview: NodePreview) -> ParsedNode {
    ParsedNode {
        preview,
        outbound: None,
        content_digest: None,
        identity_fingerprint: None,
        provider_metadata_id: None,
    }
}

pub(super) fn parse_document(
    value: Value,
) -> Result<(Vec<ParsedNode>, Vec<ParseReason>), ParseError> {
    let object = value.as_object().ok_or_else(ParseError::document)?;
    let definitions = object
        .get("outbounds")
        .and_then(Value::as_array)
        .ok_or_else(ParseError::document)?;
    if definitions.len() > MAX_NODES {
        return Err(ParseError::limit());
    }
    let mut nodes = Vec::new();
    let mut ignored = false;
    let nonproxy = BTreeSet::from(["direct", "block", "dns", "selector", "urltest"]);
    for value in definitions {
        if value
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| nonproxy.contains(kind))
        {
            ignored = true;
            continue;
        }
        let name = value.get("tag").and_then(Value::as_str);
        let mut definition = value.clone();
        let provider = super::take_provider_id(&mut definition);
        nodes.push(attach_provider(
            node(definition, nodes.len(), name),
            provider,
        ));
    }
    let mut warnings = Vec::new();
    if ignored || object.keys().any(|key| key != "outbounds") {
        warnings.push(ParseReason::new(
            "global_configuration_ignored",
            "只导入具体代理节点；原全局配置与选择组未导入",
        ));
    }
    Ok((nodes, warnings))
}

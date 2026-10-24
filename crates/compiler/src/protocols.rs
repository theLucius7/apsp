use crate::{Access, CompileError, Node, invalid_node, stat_name, valid_dns_name};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProtocolConfig {
    #[default]
    VlessReality,
    Hysteria2 {
        tls: TlsConfig,
    },
    Shadowsocks2022 {
        method: SsMethod,
        password: String,
    },
    Tuic {
        tls: TlsConfig,
    },
    Anytls {
        tls: TlsConfig,
    },
    Naive {
        tls: TlsConfig,
    },
    SnellV6 {
        psk: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SsMethod {
    #[default]
    #[serde(rename = "2022-blake3-aes-128-gcm")]
    Aes128,
    #[serde(rename = "2022-blake3-aes-256-gcm")]
    Aes256,
}

impl SsMethod {
    pub fn key_size(self) -> usize {
        match self {
            Self::Aes128 => 16,
            Self::Aes256 => 32,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TlsConfig {
    Manual {
        certificate: String,
        key: String,
    },
    Acme {
        email: String,
        challenge: AcmeChallenge,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AcmeChallenge {
    #[serde(rename = "http-01")]
    Http01,
    #[serde(rename = "tls-alpn-01")]
    TlsAlpn01,
}

impl AcmeChallenge {
    pub fn port(self) -> u16 {
        match self {
            Self::Http01 => 80,
            Self::TlsAlpn01 => 443,
        }
    }
}

impl ProtocolConfig {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::VlessReality => "vless-reality",
            Self::Hysteria2 { .. } => "hysteria2",
            Self::Shadowsocks2022 { .. } => "shadowsocks2022",
            Self::Tuic { .. } => "tuic",
            Self::Anytls { .. } => "anytls",
            Self::Naive { .. } => "naive",
            Self::SnellV6 { .. } => "snell-v6",
        }
    }

    pub fn is_reality(&self) -> bool {
        matches!(self, Self::VlessReality)
    }

    pub fn tls(&self) -> Option<&TlsConfig> {
        match self {
            Self::Hysteria2 { tls }
            | Self::Tuic { tls }
            | Self::Anytls { tls }
            | Self::Naive { tls } => Some(tls),
            _ => None,
        }
    }

    pub fn uses_tcp(&self) -> bool {
        !matches!(self, Self::Hysteria2 { .. } | Self::Tuic { .. })
    }

    pub fn credential_size(&self) -> usize {
        match self {
            Self::VlessReality => 0,
            Self::Shadowsocks2022 { method, .. } => method.key_size(),
            _ => 32,
        }
    }
}

pub(crate) fn validate(node: &Node) -> Result<(), CompileError> {
    let fail = |reason| invalid_node(node, reason);
    if node.protocol_config.is_reality() || node.protocol_config.tls().is_some() {
        if !valid_dns_name(&node.sni) || node.sni.parse::<std::net::IpAddr>().is_ok() {
            return Err(fail("sni must be a DNS hostname"));
        }
    } else if !node.sni.is_empty() {
        return Err(fail("this protocol does not use sni"));
    }
    match &node.protocol_config {
        ProtocolConfig::VlessReality => {
            if !crate::valid_key(&node.private_key) || !crate::valid_key(&node.public_key) {
                return Err(fail(
                    "Reality keys must encode 32 bytes as unpadded URL-safe base64",
                ));
            }
            if node.short_id.is_empty()
                || node.short_id.len() > 16
                || !node.short_id.len().is_multiple_of(2)
                || !node.short_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(fail(
                    "short_id must contain an even number of hexadecimal digits between 2 and 16",
                ));
            }
        }
        ProtocolConfig::Shadowsocks2022 { method, password } => {
            if !valid_secret(password, method.key_size()) {
                return Err(fail("invalid Shadowsocks server key length or encoding"));
            }
        }
        ProtocolConfig::SnellV6 { psk } if !valid_secret(psk, 32) => {
            return Err(fail("invalid Snell server key"));
        }
        _ => {}
    }
    match node.protocol_config.tls() {
        Some(TlsConfig::Manual { certificate, key }) => {
            if certificate.len() > 65536
                || key.len() > 16384
                || !certificate.contains("-----BEGIN CERTIFICATE-----")
                || !key.contains("PRIVATE KEY-----")
            {
                return Err(fail(
                    "manual TLS requires a PEM certificate chain and private key",
                ));
            }
        }
        Some(TlsConfig::Acme { email, .. }) => {
            if email.len() > 254
                || email.chars().any(|c| c.is_whitespace() || c.is_control())
                || !email.split_once('@').is_some_and(|(local, domain)| {
                    !local.is_empty()
                        && valid_dns_name(domain)
                        && domain.contains('.')
                        && !domain.contains('@')
                })
            {
                return Err(fail("ACME requires a valid contact email"));
            }
            if !node.sni.contains('.') {
                return Err(fail("ACME requires a public DNS hostname"));
            }
        }
        None => {}
    }
    let mut credentials = std::collections::BTreeSet::new();
    let mut uuids = std::collections::BTreeSet::new();
    for access in &node.users {
        if matches!(
            node.protocol_config,
            ProtocolConfig::VlessReality | ProtocolConfig::Tuic { .. }
        ) && !uuids.insert(access.uuid)
        {
            return Err(fail("authorization UUIDs must be unique within a node"));
        }
        if !node.protocol_config.is_reality()
            && !(matches!(node.protocol_config, ProtocolConfig::Hysteria2 { .. })
                && !access.credential.is_empty()
                && access.credential.len() <= 256
                && !access.credential.chars().any(char::is_control))
            && !valid_secret(&access.credential, node.protocol_config.credential_size())
        {
            return Err(fail("invalid access credential length or encoding"));
        }
        if !node.protocol_config.is_reality() && !credentials.insert(&access.credential) {
            return Err(fail(
                "authorization credentials must be unique within a node",
            ));
        }
    }
    Ok(())
}

fn valid_secret(value: &str, size: usize) -> bool {
    STANDARD
        .decode(value)
        .is_ok_and(|bytes| bytes.len() == size && STANDARD.encode(bytes) == value)
}

pub(crate) fn server_tls(node: &Node, tls: &TlsConfig) -> Value {
    let mut result = json!({"enabled": true, "server_name": node.sni});
    if matches!(
        node.protocol_config,
        ProtocolConfig::Tuic { .. } | ProtocolConfig::Hysteria2 { .. }
    ) {
        result["alpn"] = json!(["h3"]);
    }
    match tls {
        TlsConfig::Manual { certificate, key } => {
            result["certificate"] = json!(certificate);
            result["key"] = json!(key);
        }
        TlsConfig::Acme { .. } => result["certificate_provider"] = json!("managed-tls"),
    }
    result
}

pub(crate) fn client_tls(node: &Node, tls: &TlsConfig) -> Value {
    let mut result = json!({"enabled": true, "server_name": node.sni});
    if matches!(
        node.protocol_config,
        ProtocolConfig::Tuic { .. } | ProtocolConfig::Hysteria2 { .. }
    ) {
        result["alpn"] = json!(["h3"]);
    }
    if let TlsConfig::Manual { certificate, .. } = tls {
        result["certificate"] = json!(certificate);
    }
    result
}

pub(crate) fn server(node: &Node, users: &[&Access]) -> Value {
    let mut result =
        json!({"tag": format!("node-{}", node.id), "listen": "::", "listen_port": node.port});
    let entries: Vec<_> = users
        .iter()
        .map(|access| {
            let name = stat_name(access.user_id, node.id);
            match &node.protocol_config {
                ProtocolConfig::VlessReality => reality_identity(node, name, access.uuid),
                ProtocolConfig::Tuic { .. } => {
                    json!({"name": name, "uuid": access.uuid, "password": access.credential})
                }
                ProtocolConfig::Naive { .. } => {
                    json!({"username": name, "password": access.credential})
                }
                ProtocolConfig::SnellV6 { .. } => {
                    json!({"name": name, "userkey": access.credential})
                }
                _ => json!({"name": name, "password": access.credential}),
            }
        })
        .collect();
    result["users"] = json!(entries);
    result["type"] = json!(native_type(&node.protocol_config));
    match &node.protocol_config {
        ProtocolConfig::VlessReality => {
            result["tls"] = json!({"enabled": true, "server_name": node.sni, "reality": {
                "enabled": true, "handshake": {"server": node.sni, "server_port": 443},
                "private_key": node.private_key, "short_id": [node.short_id]
            }})
        }
        ProtocolConfig::Shadowsocks2022 { method, password } => {
            result["method"] = json!(method);
            result["password"] = json!(password);
        }
        ProtocolConfig::SnellV6 { psk } => {
            result["version"] = json!(6);
            result["psk"] = json!(psk);
        }
        ProtocolConfig::Naive { .. } => result["network"] = json!("tcp"),
        _ => {}
    }
    if let Some(tls) = node.protocol_config.tls() {
        result["tls"] = server_tls(node, tls);
    }
    crate::settings::apply(node, &mut result, false);
    result
}

pub(crate) fn client(node: &Node, access: &Access) -> Value {
    let mut result = json!({"type": native_type(&node.protocol_config), "tag": format!("node-{}", node.id),
        "server": crate::unbracket_host(&node.public_host), "server_port": node.public_port()});
    match &node.protocol_config {
        ProtocolConfig::VlessReality => {
            result["uuid"] = json!(access.uuid);
            if node.settings.reality.flow == crate::RealityFlow::Vision {
                result["flow"] = json!("xtls-rprx-vision");
            }
            result["tls"] = json!({"enabled": true, "server_name": node.sni,
                "utls": {"enabled": true, "fingerprint": "chrome"},
                "reality": {"enabled": true, "public_key": node.public_key, "short_id": node.short_id}});
        }
        ProtocolConfig::Shadowsocks2022 { method, password } => {
            result["method"] = json!(method);
            result["password"] = json!(format!("{password}:{}", access.credential));
        }
        ProtocolConfig::SnellV6 { psk } => {
            result["version"] = json!(6);
            result["psk"] = json!(psk);
            result["userkey"] = json!(access.credential);
        }
        _ => {
            result["password"] = json!(access.credential);
            if matches!(node.protocol_config, ProtocolConfig::Tuic { .. }) {
                result["uuid"] = json!(access.uuid);
            }
            if matches!(node.protocol_config, ProtocolConfig::Naive { .. }) {
                result["username"] = json!(stat_name(access.user_id, node.id));
                result["udp_over_tcp"] = json!({"enabled": true});
            }
        }
    }
    if let Some(tls) = node.protocol_config.tls() {
        result["tls"] = client_tls(node, tls);
    }
    crate::settings::apply(node, &mut result, true);
    result
}

fn native_type(protocol: &ProtocolConfig) -> &str {
    match protocol {
        ProtocolConfig::VlessReality => "vless",
        ProtocolConfig::Shadowsocks2022 { .. } => "shadowsocks",
        ProtocolConfig::SnellV6 { .. } => "snell",
        other => other.kind(),
    }
}

pub(crate) fn reality_identity(node: &Node, name: String, uuid: uuid::Uuid) -> Value {
    let mut identity = json!({"name":name,"uuid":uuid});
    if node.settings.reality.flow == crate::RealityFlow::Vision {
        identity["flow"] = json!("xtls-rprx-vision");
    }
    identity
}

mod clash;
mod document;
mod identity;
mod outbound;
mod uri;

#[cfg(test)]
mod tests;

pub use outbound::{ExternalProtocol, NormalizedOutbound};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// v2 reads provider-assigned node ids, so v1 cache validators no longer apply.
pub const PARSER_VERSION: &str = "apsp-subscription-v2";
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_NODES: usize = 5000;
pub const MAX_DEPTH: usize = 64;
pub const MAX_SCALAR_BYTES: usize = 64 * 1024;
pub const MAX_VALUES: usize = 100_000;

#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum FormatHint {
    #[default]
    Auto,
    UriList,
    Base64UriList,
    SingBoxJson,
    ClashYaml,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionFormat {
    UriList,
    Base64UriList,
    SingBoxJson,
    ClashYaml,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Debug)]
pub struct ParseReason {
    pub code: String,
    pub message: String,
}

impl ParseReason {
    pub(super) fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ParseStatus {
    Supported,
    Unsupported,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Debug)]
pub struct NodePreview {
    pub ordinal: usize,
    pub name: String,
    pub protocol: Option<ExternalProtocol>,
    pub server: Option<String>,
    pub server_port: Option<u16>,
    pub sni: Option<String>,
    pub transport: Option<String>,
    pub parse_status: ParseStatus,
    pub unsupported_reasons: Vec<ParseReason>,
}

// Private configuration is deliberately not Debug or Serialize as a result object.
pub struct ParsedNode {
    pub preview: NodePreview,
    pub outbound: Option<NormalizedOutbound>,
    pub content_digest: Option<String>,
    pub identity_fingerprint: Option<String>,
    pub provider_metadata_id: Option<String>,
}

pub struct ParsedSubscription {
    pub format: SubscriptionFormat,
    pub parser_version: &'static str,
    pub raw_digest: String,
    pub nodes: Vec<ParsedNode>,
    pub supported_count: usize,
    pub unsupported_count: usize,
    pub warnings: Vec<ParseReason>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParseError {
    pub code: &'static str,
    pub message: &'static str,
}

impl ParseError {
    pub(super) const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }

    pub(super) const fn limit() -> Self {
        Self::new("resource_limit", "订阅内容超过解析资源限制")
    }

    pub(super) const fn document() -> Self {
        Self::new("invalid_document", "订阅内容格式无效")
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for ParseError {}

pub fn parse_subscription(body: &[u8], hint: FormatHint) -> Result<ParsedSubscription, ParseError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(ParseError::limit());
    }
    let text = std::str::from_utf8(body).map_err(|_| ParseError::document())?;
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Err(ParseError::document());
    }
    let mut format = match hint {
        FormatHint::UriList => SubscriptionFormat::UriList,
        FormatHint::Base64UriList => SubscriptionFormat::Base64UriList,
        FormatHint::SingBoxJson => SubscriptionFormat::SingBoxJson,
        FormatHint::ClashYaml => SubscriptionFormat::ClashYaml,
        FormatHint::Auto
            if text.strip_prefix('{').is_some_and(|text| {
                text.trim_start().starts_with("proxies:")
                    || text.trim_start().starts_with("payload:")
            }) =>
        {
            SubscriptionFormat::ClashYaml
        }
        FormatHint::Auto if text.starts_with('{') || text.starts_with('[') => {
            SubscriptionFormat::SingBoxJson
        }
        FormatHint::Auto
            if text.starts_with("proxies:")
                || text.starts_with("payload:")
                || text.starts_with("---")
                || text.contains("\nproxies:")
                || text.contains("\npayload:")
                || text.contains("proxy-providers:")
                || text.lines().any(|line| {
                    line.trim_start().starts_with("proxies:")
                        || line.trim_start().starts_with("payload:")
                }) =>
        {
            SubscriptionFormat::ClashYaml
        }
        FormatHint::Auto
            if text
                .lines()
                .filter(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
                .all(|line| line.trim().contains("://")) =>
        {
            SubscriptionFormat::UriList
        }
        FormatHint::Auto => SubscriptionFormat::Base64UriList,
    };
    let (nodes, warnings) = match format {
        SubscriptionFormat::UriList => (uri::parse_list(text)?, Vec::new()),
        SubscriptionFormat::Base64UriList => {
            let decoded = uri::decode_base64(text)?;
            let decoded = std::str::from_utf8(&decoded).map_err(|_| ParseError::document())?;
            (uri::parse_list(decoded)?, Vec::new())
        }
        SubscriptionFormat::SingBoxJson => {
            let document = document::json(text)?;
            if hint == FormatHint::Auto
                && document.get("outbounds").is_none()
                && ["proxies", "payload", "proxy-providers"]
                    .iter()
                    .any(|key| document.get(*key).is_some())
            {
                format = SubscriptionFormat::ClashYaml;
                clash::parse_document(document)?
            } else {
                outbound::parse_document(document)?
            }
        }
        SubscriptionFormat::ClashYaml => clash::parse_document(document::yaml(text)?)?,
    };
    if nodes.is_empty()
        && matches!(
            format,
            SubscriptionFormat::UriList | SubscriptionFormat::Base64UriList
        )
    {
        return Err(ParseError::new(
            "no_proxy_nodes",
            "订阅内容没有具体代理节点",
        ));
    }
    let supported_count = nodes
        .iter()
        .filter(|node| node.preview.parse_status == ParseStatus::Supported)
        .count();
    Ok(ParsedSubscription {
        format,
        parser_version: PARSER_VERSION,
        raw_digest: hex_digest(body),
        unsupported_count: nodes.len() - supported_count,
        supported_count,
        nodes,
        warnings,
    })
}

/// Reads one stored sing-box outbound of a numbered source as if it were a
/// node of a sing-box JSON subscription. The display name was already checked
/// by numbered sources and is kept as shown there.
pub fn import_outbound(config: serde_json::Value, ordinal: usize, name: &str) -> ParsedNode {
    let mut parsed = outbound::node(config, ordinal, Some(name));
    parsed.preview.name = name.to_owned();
    parsed
}

pub(super) fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Removes a provider-assigned node id. The rules match numbered sources so
/// that their provider keys stay valid after migration.
pub(super) fn take_provider_id(
    value: &mut serde_json::Value,
) -> Result<Option<String>, ParseReason> {
    let Some(object) = value.as_object_mut() else {
        return Ok(None);
    };
    match object.remove("provider_id") {
        None => Ok(None),
        Some(serde_json::Value::String(id))
            if !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control) =>
        {
            Ok(Some(id))
        }
        Some(_) => Err(ParseReason::new(
            "invalid_provider_identity",
            "节点的提供方编号无效",
        )),
    }
}

pub(super) fn display_name(value: Option<&str>, ordinal: usize) -> String {
    value
        .map(str::trim)
        .filter(|name| {
            !name.is_empty()
                && name.chars().count() <= 256
                && !name.chars().any(char::is_control)
                && !name.contains("://")
                && !name.contains(['/', '\\', '?', '#', '@', '=', '&', '%'])
        })
        .map(str::to_owned)
        .unwrap_or_else(|| format!("节点 {}", ordinal + 1))
}

pub(super) fn redact_auth_name(name: &str, value: &serde_json::Value, ordinal: usize) -> String {
    fn contains_secret(name: &str, value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(values) => values.iter().any(|(key, value)| {
                let sensitive = [
                    "uuid",
                    "password",
                    "username",
                    "client_key",
                    "private_key",
                    "authorization",
                    "proxy-authorization",
                    "cookie",
                    "short_id",
                    "token",
                ]
                .contains(&key.to_ascii_lowercase().as_str());
                sensitive
                    && match value {
                        serde_json::Value::String(secret) => {
                            !secret.is_empty()
                                && (name.eq_ignore_ascii_case(secret)
                                    || secret.len() >= 8 && name.contains(secret))
                        }
                        serde_json::Value::Array(values) => values
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .any(|secret| {
                                !secret.is_empty()
                                    && (name == secret
                                        || secret.len() >= 8 && name.contains(secret))
                            }),
                        _ => false,
                    }
                    || contains_secret(name, value)
            }),
            serde_json::Value::Array(values) => {
                values.iter().any(|value| contains_secret(name, value))
            }
            _ => false,
        }
    }
    if contains_secret(name, value) {
        format!("节点 {}", ordinal + 1)
    } else {
        name.into()
    }
}

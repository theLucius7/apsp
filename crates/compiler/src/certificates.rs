use crate::{CompileError, Node, TlsConfig, invalid_node};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// One provider owns challenge listeners and persistent state for the whole service.
pub(crate) fn provider(nodes: &[&Node]) -> Result<Option<Value>, CompileError> {
    let mut settings = None;
    let mut domains = BTreeSet::new();
    for node in nodes {
        let Some(TlsConfig::Acme { email, challenge }) = node.protocol_config.tls() else {
            continue;
        };
        if let Some(previous) = settings
            && previous != (email, challenge)
        {
            return Err(invalid_node(
                node,
                "ACME email and challenge must match across the server",
            ));
        }
        settings = Some((email, challenge));
        if node.enabled && !node.users.is_empty() {
            domains.insert(node.sni.to_ascii_lowercase());
        }
    }
    let Some((email, challenge)) = settings else {
        return Ok(None);
    };
    for node in nodes {
        if node.protocol_config.uses_tcp() && node.port == challenge.port() {
            return Err(invalid_node(
                node,
                "TCP listener conflicts with the ACME challenge port",
            ));
        }
    }
    if domains.is_empty() {
        return Ok(None);
    }
    Ok(Some(json!({
        "type": "acme", "tag": "managed-tls", "domain": domains,
        "data_directory": "certificates", "email": email, "provider": "letsencrypt",
        "disable_http_challenge": challenge.port() != 80,
        "disable_tls_alpn_challenge": challenge.port() != 443
    })))
}

pub(crate) fn dns_providers(nodes: &[&Node]) -> Vec<Value> {
    let mut configs = Vec::new();
    let mut providers = Vec::new();
    for node in nodes.iter().filter(|n| n.enabled && !n.users.is_empty()) {
        let Some(
            tls @ TlsConfig::Dns01 {
                email,
                api_token,
                domains,
            },
        ) = node.protocol_config.tls()
        else {
            continue;
        };
        if configs.contains(&tls) {
            continue;
        }
        configs.push(tls);
        providers.push(json!({"type":"acme","tag":format!("managed-dns-{}",node.id),"domain":domains,"data_directory":"certificates","email":email,"provider":"letsencrypt","disable_http_challenge":true,"disable_tls_alpn_challenge":true,"dns01_challenge":{"provider":"cloudflare","api_token":api_token}}));
    }
    providers
}

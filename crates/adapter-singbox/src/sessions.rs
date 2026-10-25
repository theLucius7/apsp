//! Session evidence for concurrent client address limits.
//!
//! The runtime logs `inbound connection from <address>` and
//! `[<name>] inbound connection to <destination>` for one session under the
//! same log ID; only the address and the accounting name are kept, never the
//! destination. Sessions are listed and closed through the loopback control
//! interface compiled into the configuration.
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::{ActiveSession, BoxFuture, Prepared, SessionEvidence, SessionSource};
use serde_json::Value;
use std::{net::SocketAddr, sync::OnceLock, time::Duration};

const RESPONSE_LIMIT: usize = 16 * 1024 * 1024;
const CLOSE_BATCH_LIMIT: usize = 256;

pub(crate) struct Sessions;

pub(crate) static SESSIONS: Sessions = Sessions;

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(3))
            .build()
            .expect("static loopback client configuration")
    })
}

/// Only a loopback controller with a secret is ever contacted.
fn control(runtime: &Prepared) -> Result<Option<(SocketAddr, String)>> {
    let config: Value = serde_json::from_str(
        runtime
            .spec
            .files
            .get("config.json")
            .context("missing config.json")?,
    )?;
    let Some(api) = config["experimental"].get("clash_api") else {
        return Ok(None);
    };
    let address: SocketAddr = api["external_controller"]
        .as_str()
        .context("session control listener is missing")?
        .parse()
        .context("session control listener is not an address")?;
    ensure!(
        address.ip().is_loopback(),
        "session control listener must be loopback"
    );
    let secret = api["secret"]
        .as_str()
        .filter(|secret| secret.len() >= 32)
        .context("session control secret is missing")?;
    Ok(Some((address, secret.to_owned())))
}

/// Terminal colors the runtime adds to level labels and log identifiers.
fn without_colors(line: &str) -> std::borrow::Cow<'_, str> {
    if !line.contains('\x1b') {
        return line.into();
    }
    let mut plain = String::with_capacity(line.len());
    let mut characters = line.chars();
    while let Some(character) = characters.next() {
        if character == '\x1b' {
            // CSI sequence: ESC '[' parameters, ended by a letter.
            for next in characters.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }
    plain.into()
}

fn evidence(line: &str) -> Option<SessionEvidence> {
    let line = without_colors(line);
    // `INFO [<log id> <elapsed>] inbound/<type>[<tag>]: <message>`; the
    // timestamp prefix is not used.
    let rest = &line[line.find("INFO [")? + "INFO [".len()..];
    let digits = rest.find(|c: char| !c.is_ascii_digit())?;
    if digits == 0 || !rest[digits..].starts_with(' ') {
        return None;
    }
    let log_id = rest[..digits].to_owned();
    let rest = &rest[rest.find("] ")? + 2..];
    if !rest.starts_with("inbound/") {
        return None;
    }
    let message = &rest[rest.find("]: ")? + 3..];
    if let Some(address) = message.strip_prefix("inbound connection from ") {
        return Some(SessionEvidence {
            log_id,
            source: Some(address.trim().parse().ok()?),
            principal: None,
        });
    }
    let named = message.strip_prefix('[')?;
    let end = named.find("] ")?;
    let principal = &named[..end];
    if !principal_valid(principal) {
        return None;
    }
    let message = &named[end + 2..];
    let source = if let Some(address) = message.strip_prefix("inbound packet connection from ") {
        Some(address.trim().parse().ok()?)
    } else if message.starts_with("inbound connection to ")
        || message.starts_with("inbound packet connection to ")
        || message.starts_with("inbound packet addr connection")
    {
        None
    } else {
        return None;
    };
    Some(SessionEvidence {
        log_id,
        source,
        principal: Some(principal.to_owned()),
    })
}

/// Mirrors the protocol's accounting-name rule without a protocol dependency.
fn principal_valid(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

async fn bounded_body(response: reqwest::Response) -> Result<Vec<u8>> {
    ensure!(
        response.content_length().unwrap_or(0) <= RESPONSE_LIMIT as u64,
        "session list exceeds the size limit"
    );
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            body.len() + chunk.len() <= RESPONSE_LIMIT,
            "session list exceeds the size limit"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn parse_sessions(body: &[u8]) -> Result<Vec<ActiveSession>> {
    let value: Value = serde_json::from_slice(body)?;
    let Some(connections) = value.get("connections") else {
        anyhow::bail!("session list has no connections field");
    };
    if connections.is_null() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for connection in connections
        .as_array()
        .context("session list is not an array")?
    {
        let id = connection["id"]
            .as_str()
            .filter(|id| (1..=128).contains(&id.len()))
            .context("session has no identifier")?;
        let metadata = &connection["metadata"];
        let (Some(ip), Some(port)) = (
            metadata["sourceIP"].as_str(),
            metadata["sourcePort"].as_str(),
        ) else {
            continue;
        };
        // Sessions the runtime opened itself have no client address.
        let (Ok(ip), Ok(port)) = (ip.parse::<std::net::IpAddr>(), port.parse::<u16>()) else {
            continue;
        };
        result.push(ActiveSession {
            id: id.to_owned(),
            source: SocketAddr::new(ip, port),
        });
    }
    Ok(result)
}

impl SessionSource for Sessions {
    fn enabled(&self, runtime: &Prepared) -> bool {
        matches!(control(runtime), Ok(Some(_)))
    }

    fn parse_log_line(&self, line: &str) -> Option<SessionEvidence> {
        evidence(line)
    }

    fn active_sessions<'a>(&'a self, runtime: &'a Prepared) -> BoxFuture<'a, Vec<ActiveSession>> {
        Box::pin(async move {
            let (address, secret) = control(runtime)?.context("session control is disabled")?;
            let response = client()
                .get(format!("http://{address}/connections"))
                .bearer_auth(secret)
                .send()
                .await
                .context("session control interface is unreachable")?;
            ensure!(
                response.status().is_success(),
                "session control interface refused the list request"
            );
            parse_sessions(&bounded_body(response).await?)
        })
    }

    fn close_sessions<'a>(
        &'a self,
        runtime: &'a Prepared,
        ids: &'a [String],
    ) -> BoxFuture<'a, usize> {
        Box::pin(async move {
            let (address, secret) = control(runtime)?.context("session control is disabled")?;
            let mut closed = 0;
            for id in ids.iter().take(CLOSE_BATCH_LIMIT) {
                ensure!(
                    id.bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
                    "invalid session identifier"
                );
                let response = client()
                    .delete(format!("http://{address}/connections/{id}"))
                    .bearer_auth(&secret)
                    .send()
                    .await
                    .context("session control interface is unreachable")?;
                if response.status().is_success() {
                    closed += 1;
                }
            }
            Ok(closed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_lines_yield_addresses_and_names_but_never_destinations() {
        let from = "+0000 2026-10-25 01:00:00 INFO [3735928559 0ms] inbound/vless[vless-in]: inbound connection from 192.0.2.7:51000";
        assert_eq!(
            evidence(from),
            Some(SessionEvidence {
                log_id: "3735928559".into(),
                source: Some("192.0.2.7:51000".parse().unwrap()),
                principal: None
            })
        );
        let to = "+0000 2026-10-25 01:00:00 INFO [3735928559 2ms] inbound/vless[vless-in]: [u43_n2] inbound connection to example.com:443";
        let named = evidence(to).unwrap();
        assert_eq!(named.principal.as_deref(), Some("u43_n2"));
        assert_eq!(named.source, None);
        assert!(!format!("{named:?}").contains("example.com"));
        let colored = "+0000 2026-10-25 01:00:00 \x1b[36mINFO\x1b[0m [\x1b[38;5;99m3735928559\x1b[0m 0ms] inbound/vless[vless-in]: inbound connection from 192.0.2.7:51000";
        assert_eq!(evidence(colored), evidence(from));
        let udp = "INFO [7 0ms] inbound/shadowsocks[ss-in]: [u5_n9] inbound packet connection from [2001:db8::5]:4000";
        assert_eq!(
            evidence(udp).unwrap().source,
            Some("[2001:db8::5]:4000".parse().unwrap())
        );
        for ignored in [
            "INFO [7 0ms] outbound/direct[direct]: outbound connection to example.com:443",
            "WARN [7 0ms] inbound/vless[vless-in]: inbound connection from 192.0.2.7:1",
            "INFO inbound/vless[vless-in]: tcp server started at [::]:443",
            "INFO [7 0ms] inbound/vless[vless-in]: [bad name] inbound connection to x:1",
            "INFO [x 0ms] inbound/vless[vless-in]: inbound connection from 192.0.2.7:1",
        ] {
            assert_eq!(evidence(ignored), None, "{ignored}");
        }
    }

    #[test]
    fn session_lists_keep_client_sessions_and_tolerate_an_empty_runtime() {
        let body = br#"{"downloadTotal":0,"uploadTotal":0,"connections":[
            {"id":"6f2c0d3a-1","metadata":{"network":"tcp","sourceIP":"192.0.2.7","sourcePort":"51000","host":"example.com"}},
            {"id":"6f2c0d3a-2","metadata":{"network":"udp","sourceIP":"2001:db8::5","sourcePort":"4000"}},
            {"id":"6f2c0d3a-3","metadata":{"network":"tcp","sourceIP":"","sourcePort":"0"}}]}"#;
        let sessions = parse_sessions(body).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[1].source, "[2001:db8::5]:4000".parse().unwrap());
        assert!(
            parse_sessions(br#"{"connections":null}"#)
                .unwrap()
                .is_empty()
        );
        assert!(parse_sessions(br#"{}"#).is_err());
    }
}

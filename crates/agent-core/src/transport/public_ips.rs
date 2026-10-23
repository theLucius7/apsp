use super::Runtime;
use crate::Config;
use anyhow::Result;
use apsp_protocol::{AgentSettings, Envelope, now_timestamp};
use futures_util::StreamExt;
use std::{net::IpAddr, time::Duration};
use tokio::sync::mpsc;

const SOURCES: [&str; 2] = ["https://ipv4.icanhazip.com/", "https://ipv6.icanhazip.com/"];

fn parse(text: &str) -> Option<String> {
    let address: IpAddr = text.trim().parse().ok()?;
    let address = match address {
        IpAddr::V6(value) => value
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(value)),
        other => other,
    };
    let public = match address {
        IpAddr::V4(value) => {
            !value.is_private()
                && !value.is_loopback()
                && !value.is_link_local()
                && !value.is_unspecified()
                && !value.is_multicast()
                && !value.is_broadcast()
        }
        IpAddr::V6(value) => {
            !value.is_loopback()
                && !value.is_unspecified()
                && !value.is_unicast_link_local()
                && !value.is_unique_local()
                && !value.is_multicast()
        }
    };
    public.then(|| address.to_string())
}

async fn fetch(client: &reqwest::Client, url: &str) -> Option<String> {
    let response = client.get(url).send().await.ok()?.error_for_status().ok()?;
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if body.len() + chunk.len() > 256 {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    parse(std::str::from_utf8(&body).ok()?)
}

pub(super) async fn run(
    config: Config,
    runtime: Runtime,
    outgoing: mpsc::Sender<Envelope>,
    retirement: std::sync::Arc<crate::retirement::Retirement>,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()?;
    loop {
        {
            let _guard = retirement.gate.read().await;
            if !retirement.requested() {
                let enabled = runtime
                    .state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
                    .get_json::<AgentSettings>("agent_settings")?
                    .unwrap_or_else(|| config.settings.clone())
                    .discover_public_ips;
                if enabled && config.settings.discover_public_ips {
                    let (v4, v6) =
                        tokio::join!(fetch(&client, SOURCES[0]), fetch(&client, SOURCES[1]));
                    let now = now_timestamp();
                    let mut records = {
                        runtime
                            .state
                            .lock()
                            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
                            .get_json::<Vec<(String, i64)>>("discovered_ips")?
                            .unwrap_or_default()
                    };
                    records.retain(|(_, expires)| *expires > now);
                    for address in [v4, v6].into_iter().flatten() {
                        let family = address.contains(':');
                        records.retain(|(value, _)| value.contains(':') != family);
                        records.push((address, now + 1800));
                    }
                    runtime
                        .state
                        .lock()
                        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
                        .set_json("discovered_ips", &records)?;
                } else {
                    runtime
                        .state
                        .lock()
                        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
                        .remove_json("discovered_ips")?;
                }
                if let Some(info) = runtime.static_info()? {
                    let _ = outgoing.try_send(Envelope::new("telemetry.static", info)?);
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ip_probe_responses_require_a_single_routable_address() {
        for invalid in [
            "127.0.0.1",
            "10.0.0.1",
            "::1",
            "fc00::1",
            "<html>error</html>",
            "192.0.2.1\n192.0.2.2",
        ] {
            assert!(parse(invalid).is_none());
        }
        assert_eq!(parse(" 192.0.2.10\n").as_deref(), Some("192.0.2.10"));
    }
}

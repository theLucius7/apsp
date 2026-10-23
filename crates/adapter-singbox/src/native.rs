use anyhow::{Context, Result, bail};
use apsp_adapter_sdk::RuntimeSpec;
use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Transport {
    Tcp,
    Udp,
}

#[derive(Clone, Debug)]
pub(crate) struct Listener {
    pub address: SocketAddr,
    pub transport: Transport,
    pub tls: Option<serde_json::Value>,
    pub obfuscation: Option<String>,
}

pub(crate) fn stats_address(address: &str) -> Result<SocketAddr> {
    let address: SocketAddr = address
        .parse()
        .context("invalid statistics listen address")?;
    if address.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) || address.port() == 0 {
        bail!("statistics API must listen on 127.0.0.1 with a nonzero port");
    }
    Ok(address)
}

pub(crate) fn listen_addresses(spec: &RuntimeSpec) -> Result<Vec<Listener>> {
    let stats = stats_address(&spec.stats_listen)?;
    if spec
        .files
        .keys()
        .any(|name| name != "config.json" && name != crate::path_probe::PLAN_FILE)
        || spec.files.len()
            != if spec.files.contains_key(crate::path_probe::PLAN_FILE) {
                2
            } else {
                1
            }
    {
        bail!("runtime bundle contains an unrecognized file");
    }
    let probe = crate::path_probe::configuration(spec)?;
    let probe_port = probe
        .as_ref()
        .map(|configuration| configuration.controller_port());
    let config: serde_json::Value = serde_json::from_str(
        spec.files
            .get("config.json")
            .context("missing config.json")?,
    )?;
    let api = &config["experimental"]["v2ray_api"];
    if api["listen"].as_str() != Some(spec.stats_listen.as_str())
        || api["stats"]["enabled"].as_bool() != Some(true)
    {
        bail!("native statistics configuration must match the runtime specification");
    }
    let mut ports = BTreeSet::new();
    let mut addresses = Vec::new();
    for inbound in config["inbounds"]
        .as_array()
        .context("inbounds must be an array")?
    {
        let kind = inbound["type"].as_str().context("missing inbound type")?;
        let transports: &[Transport] = match kind {
            "vless" | "anytls" => &[Transport::Tcp],
            "snell" if inbound["version"].as_u64() == Some(6) => &[Transport::Tcp],
            "naive" if inbound["network"].as_str() == Some("tcp") => &[Transport::Tcp],
            "hysteria2" | "tuic" => &[Transport::Udp],
            "shadowsocks" => &[Transport::Tcp, Transport::Udp],
            _ => bail!("unsupported native inbound type or transport"),
        };
        let tls = if matches!(kind, "hysteria2" | "tuic" | "anytls" | "naive") {
            let tls = inbound.get("tls").context("missing TLS configuration")?;
            anyhow::ensure!(
                tls["enabled"] == true && tls["server_name"].as_str().is_some(),
                "invalid TLS configuration"
            );
            Some(tls.clone())
        } else {
            None
        };
        let obfuscation = if let Some(obfs) = inbound.get("obfs") {
            anyhow::ensure!(
                kind == "hysteria2" && obfs["type"] == "salamander",
                "unsupported obfuscation"
            );
            let password = obfs["password"]
                .as_str()
                .context("missing obfuscation password")?;
            anyhow::ensure!(
                (8..=256).contains(&password.len()),
                "invalid obfuscation password length"
            );
            Some(password.to_owned())
        } else {
            None
        };
        let port: u16 = inbound["listen_port"]
            .as_u64()
            .context("inbound has no listen port")?
            .try_into()?;
        if port == 0 || port == stats.port() || probe_port == Some(port) || !ports.insert(port) {
            bail!("invalid or duplicate native listen port");
        }
        let listen: IpAddr = inbound["listen"]
            .as_str()
            .context("inbound has no listen address")?
            .parse()?;
        let probe = match listen {
            IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            address => address,
        };
        for transport in transports {
            addresses.push(Listener {
                address: SocketAddr::new(probe, port),
                transport: *transport,
                tls: tls.clone(),
                obfuscation: obfuscation.clone(),
            });
        }
    }
    addresses.sort_by_key(|listener| listener.address);
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_ports_are_probed_on_matching_loopback_family() {
        let spec = RuntimeSpec {
            revision: 1,
            kernel_version: "1.14.2".into(),
            config_hash: String::new(),
            binary_path: "/tmp/test-runtime".into(),
            revision_dir: "/tmp/test-revision".into(),
            stats_listen: "127.0.0.1:18085".into(),
            files: [("config.json".into(), serde_json::json!({
                "inbounds":[
                    {"type":"vless","listen":"::","listen_port":20001},
                    {"type":"vless","listen":"0.0.0.0","listen_port":20002}
                ],
                "experimental":{"v2ray_api":{"listen":"127.0.0.1:18085", "stats":{"enabled":true}}}
            }).to_string())].into(),
        };
        let addresses = listen_addresses(&spec).unwrap();
        assert!(
            addresses
                .iter()
                .any(|listener| listener.address == "[::1]:20001".parse().unwrap())
        );
        assert!(
            addresses
                .iter()
                .any(|listener| listener.address == "127.0.0.1:20002".parse().unwrap())
        );
    }
}

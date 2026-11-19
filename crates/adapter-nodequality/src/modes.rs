use super::*;

pub(super) struct Mode {
    pub name: &'static str,
    pub targets: Option<String>,
    pub ips: Option<String>,
}

pub(super) fn validate(spec: &DiagnosticSpec) -> Result<Mode> {
    let mode = spec
        .options
        .get("mode")
        .map(String::as_str)
        .unwrap_or("full");
    if !supports_modes(&spec.version)
        && spec.options.keys().any(|key| {
            matches!(
                key.as_str(),
                "mode" | "daily_targets" | "environment_section" | "node_ips"
            )
        })
    {
        bail!("legacy diagnostic artifacts do not support mode options");
    }
    if spec
        .options
        .get("environment_section")
        .is_some_and(|value| value != "true")
    {
        bail!("invalid diagnostic environment option");
    }
    if mode != "ip" && spec.options.contains_key("node_ips") {
        bail!("only official node IP diagnostics accept frozen IP addresses");
    }
    if mode == "ip" {
        if !matches!(
            spec.version.as_str(),
            NODE_QUERY_VERSION | VERSION | NATIVE_VERSION
        ) || spec.options.contains_key("daily_targets")
            || spec
                .options
                .get("network_mode")
                .is_some_and(|value| value != "low")
            || spec
                .options
                .get("upload_report")
                .is_some_and(|value| value != "false")
            || spec.timeout_secs > 90
        {
            bail!("official node IP diagnostics require r21 and a bounded private job");
        }
        let ips = spec
            .options
            .get("node_ips")
            .context("node IP addresses missing")?;
        if ips.len() > 2048 {
            bail!("node IP addresses exceed byte limit");
        }
        let addresses: Vec<String> = serde_json::from_str(ips)?;
        let mut distinct = std::collections::BTreeSet::new();
        if addresses.is_empty() || addresses.len() > 8 {
            bail!("node IP diagnostics accept one to eight addresses");
        }
        for value in addresses {
            let address: std::net::IpAddr = value.parse()?;
            let public = match address {
                std::net::IpAddr::V4(address) => {
                    let [a, b, c, _] = address.octets();
                    !matches!(a, 0 | 10 | 127 | 224..=255)
                        && !(a == 100 && (64..=127).contains(&b))
                        && !(a == 169 && b == 254)
                        && !(a == 172 && (16..=31).contains(&b))
                        && !(a == 192
                            && ((b == 0 && matches!(c, 0 | 2)) || b == 168 || (b == 88 && c == 99)))
                        && !(a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
                        && !(a == 203 && b == 0 && c == 113)
                }
                std::net::IpAddr::V6(address) => {
                    let segments = address.segments();
                    segments[0] & 0xe000 == 0x2000
                        && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
                        && !(segments[0] == 0x2001 && segments[1] == 2 && segments[2] == 0)
                        && !(segments[0] == 0x3fff && segments[1] & 0xf000 == 0)
                }
            };
            if !public || address.to_string() != value || !distinct.insert(value) {
                bail!("node IP addresses must be distinct canonical public addresses");
            }
        }
        return Ok(Mode {
            name: "ip",
            targets: None,
            ips: Some(ips.clone()),
        });
    }
    if mode == "full" {
        if spec.options.contains_key("daily_targets") {
            bail!("full diagnostics cannot accept daily targets");
        }
        return Ok(Mode {
            name: "full",
            targets: None,
            ips: None,
        });
    }
    if mode != "daily"
        || spec
            .options
            .get("network_mode")
            .is_some_and(|value| value != "low")
        || spec
            .options
            .get("upload_report")
            .is_some_and(|value| value != "false")
    {
        bail!("daily diagnostics require bounded network mode without public upload");
    }
    let targets = spec
        .options
        .get("daily_targets")
        .context("daily targets missing")?;
    if targets.len() > 8192 {
        bail!("daily targets exceed byte limit");
    }
    let value: serde_json::Value = serde_json::from_str(targets)?;
    let entries = value.as_array().context("daily targets must be an array")?;
    if entries.len() > 4 {
        bail!("daily diagnostics accept at most four targets");
    }
    for entry in entries {
        let item = entry
            .as_object()
            .context("daily target must be an object")?;
        let name = entry["name"]
            .as_str()
            .context("daily target name missing")?;
        let target = entry["target"]
            .as_str()
            .context("daily target address missing")?;
        let port = entry["port"]
            .as_u64()
            .context("daily target port missing")?;
        if item.len() != 3
            || name.trim().is_empty()
            || name.len() > 128
            || name.chars().any(char::is_control)
            || target.is_empty()
            || target.len() > 253
            || target.starts_with('-')
            || !target
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".:-".contains(&byte))
            || !(1..=65535).contains(&port)
        {
            bail!("invalid daily TCP target");
        }
    }
    Ok(Mode {
        name: "daily",
        targets: Some(targets.clone()),
        ips: None,
    })
}

use anyhow::{Result, ensure};
use reqwest::{Client, Url, redirect::Policy};
use std::{net::IpAddr, time::Duration};

fn public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let b = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && !ip.is_documentation()
                && b[0] != 0
                && b[0] < 224
                && !(b[0] == 100 && (64..=127).contains(&b[1]))
                && !(b[0] == 198 && matches!(b[1], 18 | 19))
                && !(b[0] == 192 && b[1] == 0 && b[2] == 0)
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return public_address(IpAddr::V4(mapped));
            }
            let s = ip.segments();
            !ip.is_unspecified()
                && !ip.is_loopback()
                && !ip.is_multicast()
                && s[0] & 0xfe00 != 0xfc00
                && s[0] & 0xffc0 != 0xfe80
                && !(s[0] == 0x2001 && s[1] == 0xdb8)
                && s[0] & 0xe000 == 0x2000
        }
    }
}

fn https(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && url.port_or_known_default() == Some(443)
}

fn github(url: &Url) -> bool {
    https(url)
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
}

pub(super) fn download_url(canonical: &str, mirror: &str, panel: &Url) -> Result<Url> {
    let original = Url::parse(canonical)?;
    ensure!(
        github(&original) && original.query().is_none(),
        "invalid GitHub release URL"
    );
    if mirror.is_empty() {
        return Ok(original);
    }
    let base = Url::parse(mirror)?;
    ensure!(
        https(&base)
            && base.query().is_none()
            && base.origin() != panel.origin()
            && !matches!(base.host_str(), Some("localhost"))
            && base.domain().is_some(),
        "Agent mirror must be a separate HTTPS hostname without credentials or query"
    );
    Ok(Url::parse(&format!(
        "{}/{canonical}",
        mirror.trim_end_matches('/')
    ))?)
}

pub(super) async fn download(
    mut url: Url,
    mirror: &str,
    panel: &Url,
    maximum: usize,
) -> Result<Vec<u8>> {
    ensure!(
        maximum > 0 && maximum <= 128 * 1024 * 1024,
        "Agent exceeds size limit"
    );
    let mirror = (!mirror.is_empty())
        .then(|| Url::parse(mirror))
        .transpose()?;
    for hop in 0..=5 {
        let allowed = url.origin() != panel.origin()
            && (github(&url)
                || (https(&url)
                    && mirror
                        .as_ref()
                        .is_some_and(|base| base.origin() == url.origin())));
        #[cfg(test)]
        let allowed = allowed || (url.scheme() == "http" && url.host_str() == Some("127.0.0.1"));
        ensure!(
            allowed,
            "Agent redirect is outside GitHub or configured mirror"
        );
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("Agent download requires a host"))?;
        let addresses: Vec<_> = tokio::time::timeout(
            Duration::from_secs(20),
            tokio::net::lookup_host((host, url.port_or_known_default().unwrap_or(443))),
        )
        .await??
        .collect();
        let public =
            !addresses.is_empty() && addresses.iter().all(|address| public_address(address.ip()));
        #[cfg(test)]
        let public = public || (url.scheme() == "http" && host == "127.0.0.1");
        ensure!(public, "Agent download DNS returned a non-public address");
        // Never inherit device credentials, cookies or proxy configuration.
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .resolve_to_addrs(host, &addresses)
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(120))
            .build()?;
        let mut response = client
            .get(url.clone())
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Agent release download failed"))?;
        if response.status().is_redirection() {
            ensure!(hop < 5, "too many Agent download redirects");
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or_else(|| anyhow::anyhow!("missing release redirect"))?
                .to_str()?;
            url = url.join(location)?;
            continue;
        }
        ensure!(
            response.status().is_success(),
            "Agent release returned HTTP {}",
            response.status()
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|size| size <= maximum as u64),
            "Agent exceeds signed size"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                chunk.len() <= maximum.saturating_sub(bytes.len()),
                "Agent exceeds signed size"
            );
            bytes.extend_from_slice(&chunk);
        }
        return Ok(bytes);
    }
    anyhow::bail!("Agent download did not complete")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mirror_cannot_redirect_downloads_to_panel_or_embed_credentials() {
        let panel = Url::parse("https://panel.example.com").unwrap();
        let release = "https://github.com/theLucius7/apsp/releases/download/agent-v0.3.1/agent-0.3.1-linux-musl-amd64";
        assert_eq!(download_url(release, "", &panel).unwrap().as_str(), release);
        assert_eq!(
            download_url(release, "https://mirror.example.com/", &panel)
                .unwrap()
                .as_str(),
            format!("https://mirror.example.com/{release}")
        );
        for mirror in [
            "https://panel.example.com",
            "http://mirror.example.com",
            "https://token@mirror.example.com",
            "https://mirror.example.com?secret=x",
            "https://localhost",
            "https://127.0.0.1",
        ] {
            assert!(download_url(release, mirror, &panel).is_err(), "{mirror}");
        }
    }
}

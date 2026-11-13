use anyhow::{Result, ensure};
use apsp_panel_host::{
    http::{self, BodyError, Outbound},
    net,
};
use apsp_protocol::release::{
    MAX_CHECKSUMS_BYTES, MAX_METADATA_BYTES, MAX_SIGNATURE_BYTES, RELEASE_SOURCE_REPO,
    ReleaseProof, TrustedKeys, verify_release,
};
use reqwest::Url;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::{net::IpAddr, time::Duration};

/// A public address; an IPv4-mapped IPv6 answer is judged by its IPv4 address.
fn public_address(address: IpAddr) -> bool {
    net::public_ip(address.to_canonical())
}

fn allowed(url: &Url) -> bool {
    net::plain_url(url, &["https"])
        && url.port_or_known_default() == Some(443)
        && matches!(
            url.host_str(),
            Some(
                "api.github.com"
                    | "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
}

pub(super) async fn download(value: &str, maximum: usize) -> Result<Vec<u8>> {
    let mut url = Url::parse(value)?;
    for hop in 0..=3 {
        ensure!(allowed(&url), "release download host is not allowed");
        let host = url.host_str().expect("validated host");
        let addresses: Vec<_> = tokio::time::timeout(
            Duration::from_secs(20),
            tokio::net::lookup_host((host, 443)),
        )
        .await??
        .collect();
        ensure!(
            net::check_resolved(&addresses, usize::MAX, public_address).is_ok(),
            "release download DNS returned a non-public address"
        );
        let client = Outbound {
            connect_timeout: Some(Duration::from_secs(20)),
            timeout: Some(Duration::from_secs(300)),
            https_only: true,
            pin: Some((host.to_owned(), addresses)),
            product: Some("Apsp-release-import"),
            ..Outbound::default()
        }
        .build()?;
        let response = client
            .get(url.clone())
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("release download request failed"))?;
        if response.status().is_redirection() {
            ensure!(hop < 3, "release download has too many redirects");
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or_else(|| anyhow::anyhow!("release redirect lacks location"))?
                .to_str()?;
            url = url.join(location)?;
            continue;
        }
        ensure!(
            response.status().is_success(),
            "release download returned HTTP {}",
            response.status()
        );
        return match http::body(response, maximum).await {
            Ok(bytes) => Ok(bytes),
            Err(BodyError::Read(error)) => Err(error.into()),
            Err(_) => anyhow::bail!("release download exceeds size limit"),
        };
    }
    anyhow::bail!("release redirect did not terminate")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_downloads_reject_private_targets_and_untrusted_redirects() {
        for value in [
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "169.254.169.254",
            "192.0.2.1",
            "198.18.0.1",
            "::1",
            "::ffff:10.0.0.1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
        ] {
            assert!(!public_address(value.parse().unwrap()), "{value}");
        }
        assert!(public_address("8.8.8.8".parse().unwrap()));
        assert!(public_address("2606:4700::1111".parse().unwrap()));
        assert!(public_address("::ffff:8.8.8.8".parse().unwrap()));
        for value in [
            "http://github.com/",
            "https://github.com:8443/",
            "https://github.com.example.invalid/",
            "https://user@github.com/",
            "https://localhost/",
            "https://github.com/#fragment",
        ] {
            assert!(!allowed(&Url::parse(value).unwrap()));
        }
        assert!(allowed(
            &Url::parse("https://release-assets.githubusercontent.com/file?token=example").unwrap()
        ));
    }
}

#[derive(Deserialize)]
struct RemoteAsset {
    name: String,
    size: u64,
}
#[derive(Deserialize)]
struct RemoteRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<RemoteAsset>,
}

pub(super) async fn asset(tag: &str, name: &str, maximum: usize) -> Result<Vec<u8>> {
    ensure!(super::valid_tag(tag), "invalid release tag");
    ensure!(
        apsp_protocol::release::safe_component(name),
        "invalid release asset name"
    );
    download(
        &format!("https://github.com/{RELEASE_SOURCE_REPO}/releases/download/{tag}/{name}"),
        maximum,
    )
    .await
}

pub(super) async fn release_proof(tag: &str, trusted: &TrustedKeys) -> Result<ReleaseProof> {
    ensure!(super::valid_tag(tag), "invalid release tag");
    let info: RemoteRelease = serde_json::from_slice(
        &download(
            &format!("https://api.github.com/repos/{RELEASE_SOURCE_REPO}/releases/tags/{tag}"),
            1024 * 1024,
        )
        .await?,
    )?;
    ensure!(
        !info.draft && !info.prerelease && info.tag_name == tag,
        "release is not a matching published stable release"
    );
    let proof = ReleaseProof {
        metadata_json: String::from_utf8(asset(tag, "release.json", MAX_METADATA_BYTES).await?)?,
        checksums: String::from_utf8(asset(tag, "SHA256SUMS", MAX_CHECKSUMS_BYTES).await?)?,
        signature: String::from_utf8(asset(tag, "SHA256SUMS.minisig", MAX_SIGNATURE_BYTES).await?)?,
    };
    let verified = verify_release(&proof, trusted)?;
    ensure!(verified.metadata().tag == tag, "signed tag differs");
    let expected: BTreeMap<_, _> = [
        ("release.json".to_owned(), proof.metadata_json.len() as u64),
        ("SHA256SUMS".to_owned(), proof.checksums.len() as u64),
        (
            "SHA256SUMS.minisig".to_owned(),
            proof.signature.len() as u64,
        ),
    ]
    .into_iter()
    .chain(
        verified
            .metadata()
            .artifacts
            .iter()
            .map(|entry| (entry.asset_name.clone(), entry.archive_size)),
    )
    .collect();
    let names: BTreeSet<_> = info.assets.iter().map(|entry| entry.name.clone()).collect();
    ensure!(
        names.len() == info.assets.len()
            && names.len() == expected.len() + 1
            && names.contains("install.sh"),
        "release asset set is incomplete or unexpected"
    );
    for entry in info.assets {
        if entry.name == "install.sh" {
            ensure!(
                entry.size > 0 && entry.size <= super::MAX_INSTALLER as u64,
                "installer size is invalid"
            );
        } else {
            ensure!(
                expected.get(&entry.name) == Some(&entry.size),
                "release asset set or size differs"
            );
        }
    }
    Ok(proof)
}

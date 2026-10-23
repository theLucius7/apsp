use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::Prepared;
use reqwest::{Url, redirect::Policy};
use serde_json::Value;
use std::time::Duration;

const MAX_RESPONSE: usize = 16 * 1024;

fn specification(runtime: &Prepared, scope: &str, generation: u64) -> Result<(Url, String)> {
    let checks = runtime
        .spec
        .files
        .get("path-checks.json")
        .context("missing runtime checks")?;
    ensure!(
        checks.len() <= 64 * 1024,
        "runtime checks exceed size limit"
    );
    let checks: Value = serde_json::from_str(checks)?;
    let checks = checks.as_array().context("invalid runtime checks")?;
    ensure!(checks.len() <= 128, "too many runtime checks");
    let matching: Vec<_> = checks
        .iter()
        .filter(|check| check["scope"] == scope && check["generation"].as_u64() == Some(generation))
        .collect();
    ensure!(matching.len() == 1, "runtime check is absent or ambiguous");
    let check = matching[0];
    ensure!(
        check.as_object().is_some_and(|fields| fields.len() == 4
            && fields
                .keys()
                .all(|key| matches!(key.as_str(), "scope" | "generation" | "outbound" | "url"))),
        "unknown runtime check fields"
    );
    let tag = check["outbound"]
        .as_str()
        .context("missing runtime check outbound")?;
    ensure!(
        !tag.is_empty()
            && tag.len() <= 128
            && tag
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)),
        "invalid runtime check outbound"
    );
    let target = check["url"].as_str().context("missing runtime check URL")?;
    ensure!(target.len() <= 2048, "runtime check URL exceeds size limit");
    let target = Url::parse(target)?;
    ensure!(
        target.scheme() == "https"
            && target.host_str().is_some()
            && target.username().is_empty()
            && target.password().is_none()
            && target.fragment().is_none(),
        "invalid runtime check URL"
    );
    let config: Value = serde_json::from_str(
        runtime
            .spec
            .files
            .get("config.json")
            .context("missing runtime configuration")?,
    )?;
    let api = &config["experimental"]["clash_api"];
    ensure!(
        api["external_controller"] == "127.0.0.1:18086",
        "runtime control must bind the fixed loopback endpoint"
    );
    let secret = api["secret"]
        .as_str()
        .context("missing runtime control secret")?;
    ensure!(
        (32..=256).contains(&secret.len())
            && secret
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte)),
        "invalid runtime control secret"
    );
    ensure!(
        config["outbounds"]
            .as_array()
            .is_some_and(|outbounds| outbounds
                .iter()
                .filter(|outbound| outbound["tag"] == tag)
                .count()
                == 1),
        "runtime check outbound is absent or ambiguous"
    );
    let mut url = Url::parse("http://127.0.0.1:18086/")?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("invalid local control endpoint"))?
        .extend(["proxies", tag, "delay"]);
    url.query_pairs_mut()
        .append_pair("url", target.as_str())
        .append_pair("timeout", "10000");
    Ok((url, secret.into()))
}

pub(super) async fn probe(runtime: &Prepared, scope: &str, generation: u64) -> Result<()> {
    let (url, secret) = specification(runtime, scope, generation)?;
    let config = runtime
        .spec
        .files
        .get("config.json")
        .context("missing runtime configuration")?;
    ensure!(
        tokio::fs::read(runtime.spec.revision_dir.join("config.json")).await? == config.as_bytes(),
        "applied runtime configuration changed"
    );
    query_delay(url, secret).await
}

async fn query_delay(url: Url, secret: String) -> Result<()> {
    // Never send local management credentials through an environment proxy or redirect.
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(12))
        .connect_timeout(Duration::from_secs(2))
        .build()?;
    let mut response = client.get(url).bearer_auth(secret).send().await?;
    ensure!(
        response.status().is_success()
            && response
                .content_length()
                .is_none_or(|size| size <= MAX_RESPONSE as u64),
        "runtime dependency check failed"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            chunk.len() <= MAX_RESPONSE.saturating_sub(bytes.len()),
            "runtime dependency response exceeds size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    let result: Value = serde_json::from_slice(&bytes)?;
    ensure!(
        result["delay"]
            .as_u64()
            .is_some_and(|delay| delay <= 10_000),
        "runtime dependency check returned no valid delay"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apsp_adapter_sdk::RuntimeSpec;
    use std::collections::BTreeMap;
    fn prepared() -> Prepared {
        Prepared { spec: RuntimeSpec { revision:1,kernel_version:"1.14.2".into(),config_hash:"a".repeat(64),binary_path:"/tmp/runtime".into(),revision_dir:"/tmp/config".into(),stats_listen:"127.0.0.1:18085".into(),files:BTreeMap::from([
            ("config.json".into(),serde_json::json!({"outbounds":[{"tag":"path-1-g2-h0"}],"experimental":{"clash_api":{"external_controller":"127.0.0.1:18086","secret":"TEST_ONLY_abcdefghijklmnopqrstuvwxyz"}}}).to_string()),
            ("path-checks.json".into(),serde_json::json!([{"scope":"path-1","generation":2,"outbound":"path-1-g2-h0","url":"https://panel.example.com/healthz"}]).to_string()),
        ]) }, listen_ports:vec![] }
    }
    #[test]
    fn dependency_probe_is_bound_to_exact_allowlisted_scope_and_local_control() {
        let mut runtime = prepared();
        let (url, secret) = specification(&runtime, "path-1", 2).unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert_eq!(url.path(), "/proxies/path-1-g2-h0/delay");
        assert_eq!(secret, "TEST_ONLY_abcdefghijklmnopqrstuvwxyz");
        assert!(specification(&runtime, "path-1", 3).is_err());
        assert!(specification(&runtime, "path-2", 2).is_err());
        let config = runtime.spec.files.get_mut("config.json").unwrap();
        *config = config.replace("127.0.0.1:18086", "0.0.0.0:18086");
        assert!(specification(&runtime, "path-1", 2).is_err());
    }
    #[test]
    fn dependency_probe_rejects_unknown_fields_weak_secrets_and_non_https_targets() {
        for (file, old, new) in [
            (
                "path-checks.json",
                "https://panel.example.com/healthz",
                "http://panel.example.com/healthz",
            ),
            (
                "path-checks.json",
                "https://panel.example.com/healthz",
                "https://credential@panel.example.com/healthz",
            ),
            (
                "config.json",
                "TEST_ONLY_abcdefghijklmnopqrstuvwxyz",
                "weak",
            ),
            (
                "path-checks.json",
                "\"scope\":",
                "\"command\":\"bad\",\"scope\":",
            ),
        ] {
            let mut runtime = prepared();
            let content = runtime.spec.files.get_mut(file).unwrap();
            *content = content.replace(old, new);
            assert!(specification(&runtime, "path-1", 2).is_err());
        }
    }

    #[tokio::test]
    async fn local_delay_request_is_authenticated_bounded_and_never_follows_redirects() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        for (status, body, expected) in [
            ("200 OK", r#"{"delay":42}"#.to_owned(), true),
            ("302 Found", String::new(), false),
            ("200 OK", "x".repeat(MAX_RESPONSE + 1), false),
            ("200 OK", r#"{"message":"native secret"}"#.to_owned(), false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let fixture = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut part = [0; 1024];
                    let size = stream.read(&mut part).await.unwrap();
                    assert_ne!(size, 0);
                    bytes.extend_from_slice(&part[..size]);
                    if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(bytes).unwrap();
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer test_only_secret")
                );
                assert!(request.starts_with("GET /proxies/test/delay?url=https%3A%2F%2Fpanel.example.com%2Fhealthz&timeout=10000 "));
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:1/credential-leak\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
            let url=Url::parse(&format!("http://{address}/proxies/test/delay?url=https%3A%2F%2Fpanel.example.com%2Fhealthz&timeout=10000")).unwrap();
            assert_eq!(
                query_delay(url, "TEST_ONLY_SECRET".into()).await.is_ok(),
                expected
            );
            fixture.await.unwrap();
        }
    }
}

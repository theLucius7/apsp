use anyhow::{Context, Result, bail, ensure};
use apsp_adapter_sdk::{Prepared, RuntimeProbeMeasurement, RuntimeProbePlan, RuntimeSpec};
use reqwest::{Url, header};
use std::{collections::BTreeSet, net::SocketAddr, time::Duration};

pub(crate) const PLAN_FILE: &str = "runtime-probes.json";
const MAX_PLAN: usize = 64 * 1024;

pub(crate) struct Configuration {
    plan: RuntimeProbePlan,
    controller: SocketAddr,
    secret: String,
}

impl Configuration {
    pub(crate) fn controller_port(&self) -> u16 {
        self.controller.port()
    }
}

fn uuid(value: &str) -> bool {
    value.len() == 36
        && value != "00000000-0000-0000-0000-000000000000"
        && value.bytes().enumerate().all(|(i, byte)| {
            if [8, 13, 18, 23].contains(&i) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}

fn identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn target(value: &str) -> Result<Url> {
    ensure!(
        value.len() <= 8192
            && value.trim() == value
            && !value.chars().any(char::is_control)
            && !value.contains('\\'),
        "invalid signed verification target"
    );
    let url =
        Url::parse(value).map_err(|_| anyhow::anyhow!("invalid signed verification target"))?;
    ensure!(
        url.scheme() == "https"
            && url.has_host()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && url.query().is_none()
            && url.path() == "/health"
            && url.port_or_known_default().is_some_and(|port| port != 0),
        "signed verification target must be the panel HTTPS health endpoint"
    );
    let authority = value
        .split_once("://")
        .map(|(_, tail)| tail.split('/').next().unwrap_or(""));
    ensure!(
        authority.is_some_and(|authority| !authority.contains('@')),
        "verification target credentials are prohibited"
    );
    Ok(url)
}

pub(crate) fn configuration(spec: &RuntimeSpec) -> Result<Option<Configuration>> {
    let Some(raw) = spec.files.get(PLAN_FILE) else {
        return Ok(None);
    };
    ensure!(
        raw.len() <= MAX_PLAN,
        "verification plan exceeds its byte limit"
    );
    let plan: RuntimeProbePlan = serde_json::from_str(raw)
        .map_err(|_| anyhow::anyhow!("invalid signed verification plan"))?;
    ensure!(
        plan.schema == 1
            && plan.runtime_version == "1.14.2"
            && spec.kernel_version == plan.runtime_version
            && !plan.bindings.is_empty()
            && plan.bindings.len() <= 256,
        "verification requires the exact supported runtime and a bounded plan"
    );
    ensure!(
        plan.required_build_tags.len() <= 16
            && plan
                .required_build_tags
                .iter()
                .all(|tag| identifier(tag, 64))
            && plan
                .required_build_tags
                .iter()
                .any(|tag| tag == "with_clash_api")
            && plan
                .required_build_tags
                .iter()
                .any(|tag| tag == "with_v2ray_api"),
        "verification plan omits required runtime features"
    );
    let raw = spec
        .files
        .get("config.json")
        .context("missing native configuration")?;
    let native: serde_json::Value = serde_json::from_str(raw)
        .map_err(|_| anyhow::anyhow!("invalid native verification configuration"))?;
    let api = &native["experimental"]["clash_api"];
    let controller: SocketAddr = api["external_controller"]
        .as_str()
        .context("missing verification controller")?
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid verification controller"))?;
    ensure!(
        controller.ip() == std::net::Ipv4Addr::LOCALHOST
            && controller.port() != 0
            && controller.to_string() != spec.stats_listen,
        "verification controller must use a distinct IPv4 loopback port"
    );
    let secret = api["secret"]
        .as_str()
        .context("missing verification controller authentication")?;
    ensure!(
        secret.len() == 64 && secret.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "verification controller requires a private random authentication secret"
    );
    ensure!(
        api.get("external_ui")
            .is_none_or(|ui| ui.as_str() == Some(""))
            && api
                .get("external_ui_download_url")
                .is_none_or(|url| url.as_str() == Some("")),
        "verification controller must not download or expose an external UI"
    );
    let outbounds = native["outbounds"]
        .as_array()
        .context("missing verification outbounds")?;
    let mut ids = BTreeSet::new();
    for binding in &plan.bindings {
        ensure!(
            uuid(&binding.id) && ids.insert(&binding.id) && identifier(&binding.selector, 128),
            "invalid or duplicate signed verification binding"
        );
        target(&binding.target)?;
        let matches: Vec<_> = outbounds
            .iter()
            .filter(|outbound| outbound["tag"].as_str() == Some(&binding.selector))
            .collect();
        ensure!(
            matches.len() == 1
                && matches[0]["type"].as_str().is_some_and(|kind| matches!(
                    kind,
                    "vless"
                        | "vmess"
                        | "trojan"
                        | "shadowsocks"
                        | "hysteria2"
                        | "tuic"
                        | "anytls"
                        | "socks"
                        | "http"
                )),
            "signed verification binding must select a concrete proxy outbound"
        );
    }
    Ok(Some(Configuration {
        plan,
        controller,
        secret: secret.to_owned(),
    }))
}

pub(crate) fn validate_build(spec: &RuntimeSpec, output: &str) -> Result<()> {
    if let Some(configuration) = configuration(spec)? {
        let tags = output
            .lines()
            .find_map(|line| line.strip_prefix("Tags: "))
            .context("runtime feature evidence is missing")?;
        let actual: BTreeSet<_> = tags.split(',').map(str::trim).collect();
        ensure!(
            configuration
                .plan
                .required_build_tags
                .iter()
                .all(|tag| actual.contains(tag.as_str())),
            "runtime lacks a feature required by the signed path plan"
        );
    }
    Ok(())
}

pub(crate) async fn execute(runtime: &Prepared, id: &str) -> Result<RuntimeProbeMeasurement> {
    let configuration =
        configuration(&runtime.spec)?.context("runtime has no signed verification plan")?;
    let binding = configuration
        .plan
        .bindings
        .iter()
        .find(|binding| binding.id == id)
        .context("verification identifier is absent from the signed applied plan")?;
    let target = target(&binding.target)?;
    let mut url = Url::parse(&format!("http://{}/", configuration.controller))
        .map_err(|_| anyhow::anyhow!("invalid verification controller"))?;
    url.set_path(&format!("/proxies/{}/delay", binding.selector));
    url.query_pairs_mut()
        .append_pair("url", target.as_str())
        .append_pair("timeout", "4500");
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| anyhow::anyhow!("verification transport initialization failed"))?;
    let mut authentication =
        header::HeaderValue::from_str(&format!("Bearer {}", configuration.secret))
            .map_err(|_| anyhow::anyhow!("invalid verification authentication"))?;
    authentication.set_sensitive(true);
    let mut response = client
        .get(url)
        .header(header::AUTHORIZATION, authentication)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("runtime path verification transport failed"))?;
    ensure!(
        response.status() == reqwest::StatusCode::OK,
        "runtime path verification was not successful"
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= 4096),
        "runtime verification response exceeds its budget"
    );
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("runtime verification response failed"))?
    {
        ensure!(
            body.len().saturating_add(chunk.len()) <= 4096,
            "runtime verification response exceeds its budget"
        );
        body.extend_from_slice(&chunk);
    }
    let value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| anyhow::anyhow!("invalid runtime verification response"))?;
    if value.as_object().is_none_or(|object| object.len() != 1) {
        bail!("invalid runtime verification response");
    }
    let elapsed_ms = value["delay"]
        .as_u64()
        .filter(|value| *value > 0 && *value <= 4500)
        .context("runtime verification measurement is invalid")?;
    Ok(RuntimeProbeMeasurement { elapsed_ms })
}

#[cfg(test)]
mod tests;

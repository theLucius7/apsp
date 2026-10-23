use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::DiagnosticSpec;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    net::IpAddr,
    path::{Component, Path},
};

pub(crate) const INPUT_LIMIT: usize = 16 * 1024;
pub(crate) const OUTPUT_LIMIT: usize = 64 * 1024;
pub(crate) const ENGINE_VERSION: &str = "0.3.0";
pub(crate) const BINARY: &str = "apsp-tcp-probe";
pub(crate) const SEMANTICS: &str = "连接成功率和 TCP 建连耗时；不是包丢失率、吞吐测速或上游 TcpQuality 兼容评分。地区与运营商是管理员配置标签，未指定地区不推断。";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
    pub id: String,
    pub name: String,
    pub target: String,
    pub port: u16,
    pub carrier: String,
    #[serde(default)]
    pub region: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub schema: u32,
    pub targets: Vec<Target>,
}
pub(crate) struct Validated<'a> {
    pub spec: &'a DiagnosticSpec,
    pub source: &'a str,
    pub snapshot: Snapshot,
    pub bytes: &'a str,
    pub digest: &'a str,
    pub ip_version: &'a str,
    pub count: u8,
    pub concurrency: u8,
}
pub(crate) fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}
pub(crate) fn digest(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
pub(crate) fn unicast(address: IpAddr) -> bool {
    !address.is_unspecified()
        && !address.is_multicast()
        && !matches!(address, IpAddr::V4(address) if address.is_broadcast())
}
fn region(value: &str) -> bool {
    matches!(
        value,
        "east_asia" | "southeast_asia" | "europe" | "americas" | "other"
    )
}
fn text(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}
fn host(value: &str) -> bool {
    if let Ok(address) = value.parse::<IpAddr>() {
        return unicast(address);
    }
    !value.is_empty()
        && value.len() <= 253
        && value
            .strip_suffix('.')
            .unwrap_or(value)
            .split('.')
            .all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
}
pub(crate) fn path(path: &Path) -> Result<String> {
    let value = path.to_str().context("diagnostic path is not UTF-8")?;
    ensure!(
        path.is_absolute()
            && path.parent().is_some()
            && !path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'/' | b'_' | b'.' | b'-')),
        "diagnostic paths must be absolute without expansion or traversal"
    );
    Ok(value.into())
}
impl Snapshot {
    pub fn valid(&self) -> bool {
        let mut ids = BTreeSet::new();
        self.schema == 1
            && (1..=8).contains(&self.targets.len())
            && self.targets.iter().all(|target| {
                uuid(&target.id)
                    && ids.insert(target.id.to_ascii_lowercase())
                    && text(&target.name, 128)
                    && host(&target.target)
                    && target.port > 0
                    && target.carrier.len() <= 64
                    && !target.carrier.chars().any(char::is_control)
                    && target.region.as_ref().is_none_or(|value| region(value))
            })
    }
}
pub(crate) fn validate(spec: &DiagnosticSpec) -> Result<Validated<'_>> {
    ensure!(uuid(&spec.id), "diagnostic id must be a UUID");
    path(&spec.binary_path)?;
    path(&spec.job_dir)?;
    ensure!(
        spec.binary_path.file_name().and_then(|name| name.to_str()) == Some(BINARY),
        "unexpected diagnostic binary"
    );
    ensure!(
        spec.job_dir.file_name().and_then(|name| name.to_str()) == Some(spec.id.as_str()),
        "workspace must be scoped to the diagnostic UUID"
    );
    ensure!(spec.timeout_secs > 0, "diagnostic deadline is exhausted");
    let source = spec
        .version
        .strip_prefix("0.3.0-")
        .and_then(|version| version.strip_suffix("-r1"))
        .context("unsupported TCP artifact version")?;
    ensure!(
        digest(source, 40),
        "TCP artifact must pin a lowercase source commit"
    );
    ensure!(
        spec.binary_path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some(spec.version.as_str()),
        "artifact directory does not match its version"
    );
    ensure!(
        spec.options.keys().all(|key| matches!(
            key.as_str(),
            "ip_version"
                | "count"
                | "concurrency"
                | "targets"
                | "target_digest"
                | "environment_section"
        )),
        "unsupported TCP diagnostic option"
    );
    ensure!(
        spec.options
            .get("environment_section")
            .is_none_or(|value| matches!(value.as_str(), "true" | "false")),
        "invalid core environment chapter option"
    );
    let ip_version = spec
        .options
        .get("ip_version")
        .context("IP version is required")?
        .as_str();
    ensure!(matches!(ip_version, "4" | "6"), "IP version must be 4 or 6");
    let count = match spec.options.get("count").map(String::as_str).unwrap_or("4") {
        "4" => 4,
        "8" => 8,
        _ => anyhow::bail!("count must be 4 or 8"),
    };
    let concurrency = match spec
        .options
        .get("concurrency")
        .map(String::as_str)
        .unwrap_or("1")
    {
        "1" => 1,
        "2" => 2,
        _ => anyhow::bail!("concurrency must be 1 or 2"),
    };
    let bytes = spec
        .options
        .get("targets")
        .context("frozen targets are required")?
        .as_str();
    ensure!(bytes.len() <= INPUT_LIMIT, "target snapshot exceeds 16 KiB");
    let expected = spec
        .options
        .get("target_digest")
        .context("target digest is required")?
        .as_str();
    ensure!(
        digest(expected, 64) && format!("{:x}", Sha256::digest(bytes.as_bytes())) == expected,
        "target digest mismatch"
    );
    let snapshot: Snapshot =
        serde_json::from_str(bytes).context("invalid frozen target snapshot")?;
    ensure!(snapshot.valid(), "invalid frozen target scope");
    Ok(Validated {
        spec,
        source,
        snapshot,
        bytes,
        digest: expected,
        ip_version,
        count,
        concurrency,
    })
}

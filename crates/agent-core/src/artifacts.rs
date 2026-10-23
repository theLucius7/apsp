mod cache;
mod cancellation;
mod github;
use crate::config::validate_panel_url;
mod preflight;
#[cfg(test)]
mod tests;
mod verification;
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::{Descriptor, Privileged};
use apsp_protocol::release::{ReleaseError, ReleaseProof, TrustedKeys, VerifiedArtifact};
use apsp_protocol::{
    Artifact, Bundle, DiagnosticJob, DiagnosticSectionUpdate, DiagnosticUpdate, Manifest,
};
use futures_util::StreamExt;
pub use preflight::verify_cache;
use reqwest::{Client, Url, redirect::Policy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Component, Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;
pub(crate) use verification::verify_expected;
pub use verification::{verify_installed_binary, verify_release_directory};

const MAX_DOWNLOAD: usize = 512 * 1024 * 1024;
const MAX_JSON: usize = 32 * 1024 * 1024;
const MARKER: &str = ".artifact.json";

pub(crate) fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !matches!(value, "." | "..")
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[derive(Clone)]
pub struct PanelClient {
    client: Client,
    panel: Url,
    session_token: String,
    keys: std::result::Result<TrustedKeys, ReleaseError>,
    #[cfg(test)]
    pub(crate) agent_download_test_origin: Option<Url>,
}

#[derive(Serialize, Deserialize)]
struct InstalledArtifact {
    #[serde(default)]
    proof: Option<ReleaseProof>,
}

fn normalized_hash(value: &str) -> Result<String> {
    ensure!(
        value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid SHA256 digest"
    );
    Ok(value.to_ascii_lowercase())
}

pub fn validate_bundle_files(bundle: &Bundle) -> Result<()> {
    ensure!(!bundle.files.is_empty(), "bundle cannot be empty");
    for name in bundle.files.keys() {
        let path = Path::new(name);
        ensure!(
            !name.is_empty()
                && name.len() <= 1024
                && !name.contains('\\')
                && !name.chars().any(char::is_control),
            "invalid bundle file name"
        );
        ensure!(
            !path.is_absolute()
                && name
                    .split('/')
                    .all(|part| !part.is_empty() && !matches!(part, "." | "..")),
            "bundle file must have a safe relative path"
        );
        ensure!(
            path.components()
                .all(|part| matches!(part, Component::Normal(_))),
            "invalid bundle file path"
        );
    }
    Ok(())
}

impl PanelClient {
    pub(crate) async fn agent_binary(
        &self,
        artifact: &Artifact,
        version: &str,
        mirror: &str,
    ) -> Result<Vec<u8>> {
        let proof = artifact
            .proof
            .as_ref()
            .context("Agent update has no signed release proof")?;
        let release = verification::signed_release(proof, self.keys()?)?;
        let verified = release.native_artifact("agent", version)?;
        let expected = format!(
            "https://github.com/{}/releases/download/{}/{}",
            release.metadata().source_repo,
            release.metadata().tag,
            verified.metadata().asset_name
        );
        ensure!(
            verified.metadata().format == "raw"
                && verified.metadata().binary_name == crate::system::deploy::executable_name()
                && artifact.url == expected,
            "Agent update identity differs from signed release"
        );
        ensure!(
            normalized_hash(&artifact.sha256)? == verified.sha256(),
            "Agent update SHA256 differs from signed release"
        );
        let url = github::download_url(&expected, mirror, &self.panel)?;
        #[cfg(test)]
        let url = self
            .agent_download_test_origin
            .as_ref()
            .map_or(url.clone(), |origin| origin.join(url.path()).unwrap());
        let bytes = github::download(
            url,
            mirror,
            &self.panel,
            verified.metadata().archive_size as usize,
        )
        .await?;
        verified.verify_archive(&bytes)?;
        verified.verify_binary(&bytes)?;
        Ok(bytes)
    }
    pub(crate) async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.panel.join(path)?;
        // Only this fixed panel metadata request negotiates GitHub downloads.
        // Artifact URLs retain the separate origin and query validation below.
        let bytes = if path == "/api/agent/v1/update?download_source=github" {
            self.download_panel_url(url, 1024 * 1024).await?
        } else {
            self.download(url.as_str(), 1024 * 1024).await?
        };
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub(crate) async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        value: &impl Serialize,
    ) -> Result<T> {
        let response = self
            .client
            .post(self.panel.join(path)?)
            .bearer_auth(&self.session_token)
            .json(value)
            .send()
            .await?;
        ensure!(
            response.status().is_success(),
            "Agent request returned HTTP {}",
            response.status()
        );
        Ok(serde_json::from_slice(
            &Self::response_bytes(response, 16 * 1024).await?,
        )?)
    }

    async fn response_bytes(response: reqwest::Response, maximum: usize) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            ensure!(
                chunk.len() <= maximum.saturating_sub(bytes.len()),
                "Agent response exceeds size limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    pub fn new(panel_url: &str, session_token: &str) -> Result<Self> {
        Self::with_panel_ca(panel_url, session_token, None)
    }

    pub fn from_config(config: &crate::Config, session_token: &str) -> Result<Self> {
        Self::with_panel_ca(
            &config.panel_url,
            session_token,
            config.panel_ca_file.as_deref(),
        )
    }

    fn with_panel_ca(panel_url: &str, session_token: &str, ca: Option<&Path>) -> Result<Self> {
        ensure!(
            !session_token.is_empty()
                && session_token.len() <= 512
                && !session_token.chars().any(char::is_control),
            "invalid device session token"
        );
        Ok(Self {
            client: crate::panel_tls::client_builder(ca)?
                .redirect(Policy::none())
                .connect_timeout(Duration::from_secs(20))
                .timeout(Duration::from_secs(120))
                .build()?,
            panel: validate_panel_url(panel_url)?,
            session_token: session_token.to_owned(),
            keys: TrustedKeys::compiled(),
            #[cfg(test)]
            agent_download_test_origin: None,
        })
    }

    /// Explicit trusted roots for an embedding caller or deterministic tests.
    pub fn with_trusted_keys(mut self, keys: TrustedKeys) -> Self {
        self.keys = Ok(keys);
        self
    }

    pub(crate) fn keys(&self) -> Result<&TrustedKeys> {
        self.keys
            .as_ref()
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    pub(crate) fn verify_artifact(
        &self,
        artifact: &Artifact,
        version: &str,
        descriptor: &Descriptor,
    ) -> Result<VerifiedArtifact> {
        let url = self.validate_url(&artifact.url)?;
        let proof = artifact
            .proof
            .as_ref()
            .context("artifact has no signed release proof")?;
        let verified = verification::signed_artifact(proof, descriptor, version, self.keys()?)?;
        ensure!(
            url.path() == format!("/api/agent/v1/artifacts/{}", verified.path()),
            "artifact URL differs from signed identity"
        );
        ensure!(
            normalized_hash(&artifact.sha256)? == verified.sha256(),
            "artifact digest differs from signed release"
        );
        Ok(verified)
    }

    fn validate_url(&self, value: &str) -> Result<Url> {
        let url = Url::parse(value).context("invalid download URL")?;
        ensure!(
            url.origin() == self.panel.origin()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "downloads must use the configured panel origin without credentials, query or fragment"
        );
        Ok(url)
    }

    async fn download(&self, value: &str, maximum: usize) -> Result<Vec<u8>> {
        let url = self.validate_url(value)?;
        self.download_panel_url(url, maximum).await
    }

    async fn download_panel_url(&self, url: Url, maximum: usize) -> Result<Vec<u8>> {
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.session_token)
            .send()
            .await?;
        ensure!(
            response.status().is_success(),
            "panel download returned HTTP {}",
            response.status()
        );
        if let Some(length) = response.content_length() {
            ensure!(length <= maximum as u64, "download exceeds size limit");
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            ensure!(
                chunk.len() <= maximum.saturating_sub(body.len()),
                "download exceeds size limit"
            );
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    pub async fn manifest(&self) -> Result<Manifest> {
        let url = self.panel.join("/api/agent/v1/manifest")?;
        Ok(serde_json::from_slice(
            &self.download(url.as_str(), MAX_JSON).await?,
        )?)
    }

    pub async fn diagnostic_jobs(&self) -> Result<Vec<DiagnosticJob>> {
        let url = self.panel.join("/api/agent/v1/diagnostics")?;
        let jobs: Vec<DiagnosticJob> =
            serde_json::from_slice(&self.download(url.as_str(), 1024 * 1024).await?)?;
        ensure!(jobs.len() <= 64, "too many pending diagnostic jobs");
        Ok(jobs)
    }

    pub async fn agent_settings(&self) -> Result<apsp_protocol::AgentSettings> {
        let url = self.panel.join("/api/agent/v1/settings")?;
        let settings: apsp_protocol::AgentSettings =
            serde_json::from_slice(&self.download(url.as_str(), 16 * 1024).await?)?;
        ensure!(settings.valid(), "panel provided invalid Agent settings");
        Ok(settings)
    }

    pub async fn telemetry(
        &self,
        batch: &apsp_protocol::TelemetryBatch,
    ) -> Result<apsp_protocol::TelemetryAck> {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&serde_json::to_vec(batch)?)?;
        let response = self
            .client
            .post(self.panel.join("/api/agent/v1/telemetry")?)
            .bearer_auth(&self.session_token)
            .header(reqwest::header::CONTENT_ENCODING, "gzip")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(encoder.finish()?)
            .send()
            .await?;
        ensure!(
            response.status().is_success(),
            "telemetry upload returned HTTP {}",
            response.status()
        );
        let body = Self::response_bytes(response, 16 * 1024).await?;
        ensure!(
            body.len() <= 16 * 1024,
            "telemetry acknowledgment exceeds size limit"
        );
        let ack: apsp_protocol::TelemetryAck = serde_json::from_slice(&body)?;
        ensure!(
            ack.ids.len() <= 64
                && ack
                    .ids
                    .iter()
                    .all(|id| batch.samples.iter().any(|sample| sample.id == *id)),
            "panel acknowledged samples outside the submitted batch"
        );
        Ok(ack)
    }

    pub async fn diagnostic_section(&self, update: &DiagnosticSectionUpdate) -> Result<()> {
        ensure!(update.valid(), "invalid diagnostic chapter");
        let response = self
            .client
            .post(
                self.panel
                    .join(&format!("/api/agent/v1/diagnostics/{}/sections", update.id))?,
            )
            .bearer_auth(&self.session_token)
            .json(update)
            .send()
            .await?;
        ensure!(
            response.status().is_success(),
            "diagnostic chapter returned HTTP {}",
            response.status()
        );
        Ok(())
    }

    pub async fn diagnostic_update(&self, update: &DiagnosticUpdate) -> Result<()> {
        let url = self
            .panel
            .join(&format!("/api/agent/v1/diagnostics/{}", update.id))?;
        let response = self
            .client
            .post(url)
            .bearer_auth(&self.session_token)
            .json(update)
            .send()
            .await?;
        ensure!(
            response.status().is_success(),
            "diagnostic update returned HTTP {}",
            response.status()
        );
        Ok(())
    }

    pub async fn bundle(&self, url: &str, expected_sha256: &str) -> Result<Bundle> {
        let expected = normalized_hash(expected_sha256)?;
        let bytes = self.download(url, MAX_JSON).await?;
        ensure!(digest(&bytes) == expected, "bundle SHA256 mismatch");
        let bundle = serde_json::from_slice(&bytes)?;
        validate_bundle_files(&bundle)?;
        Ok(bundle)
    }
}

async fn ensure_ordinary_directory_if_present(path: &Path) -> Result<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => ensure!(
            metadata.is_dir(),
            "artifact plugin path is not an ordinary directory"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) async fn verify_expected_agent(
    binary: &Path,
    version: &str,
    keys: &TrustedKeys,
) -> Result<()> {
    ensure!(
        binary
            .parent()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            == Some(version),
        "Agent update version directory mismatch"
    );
    verification::verify_binary_with_keys(binary, "agent", "raw", keys).await
}

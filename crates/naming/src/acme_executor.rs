use super::{
    acme::{BUILTIN_LEGO_PATH, BUILTIN_LEGO_VERSION, Plan, version_mismatch_remediation},
    documents,
    models::Configuration,
    x509,
};
// Keep crate-root governance re-export referenced while naming/src/lib.rs re-exports releases.
use crate::{AppState, error::ApiError, releases as _};
use anyhow::{Context, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::{
    collections::BTreeMap,
    future::Future,
    path::{Component, Path, PathBuf},
    pin::Pin,
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExecutionFailure {
    pub code: String,
    pub before_execution: bool,
    pub detail: Option<String>,
    pub extra: Option<Value>,
}

impl ExecutionFailure {
    pub(crate) fn before(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            before_execution: true,
            detail: None,
            extra: None,
        }
    }

    pub(crate) fn before_with_detail(code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            before_execution: true,
            detail: Some(detail.into()),
            extra: None,
        }
    }

    pub(crate) fn after(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            before_execution: false,
            detail: None,
            extra: None,
        }
    }

    fn from_pre_api(error: ApiError, fallback_code: &'static str) -> Self {
        match error {
            ApiError::NotFound => Self::before("resource_not_found"),
            ApiError::Unauthorized => Self::before("authentication_required"),
            ApiError::Forbidden(message) => Self {
                code: "permission_denied".into(),
                before_execution: true,
                detail: Some(message),
                extra: None,
            },
            ApiError::Busy => Self::before("issuer_busy"),
            ApiError::BadRequest(message) | ApiError::Conflict(message) => {
                Self::before_with_detail(fallback_code, message)
            }
            ApiError::ConflictReferences { message, .. } => {
                Self::before_with_detail(fallback_code, message)
            }
            ApiError::Database(_) | ApiError::Internal(_) => Self::before(fallback_code),
        }
    }

    pub(crate) fn into_result_json(self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("error_code".into(), json!(self.code));
        object.insert(
            "issued".into(),
            if self.before_execution {
                json!(false)
            } else {
                json!("unknown")
            },
        );
        object.insert("saved".into(), json!(false));
        object.insert("deployed".into(), json!(false));
        object.insert("cleanup_required".into(), json!(!self.before_execution));
        if let Some(detail) = self.detail {
            object.insert("error".into(), json!(detail));
        }
        if let Some(Value::Object(extra)) = self.extra {
            for (key, value) in extra {
                object.entry(key).or_insert(value);
            }
        }
        Value::Object(object)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinaryFormat {
    Elf64Le { e_machine: u16 },
    MachO64Le { cpu_type: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BuiltinTargetSpec {
    pub os: &'static str,
    pub arch: &'static str,
    pub upstream_platform: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    pub format: BinaryFormat,
}

/// Pinned official `go-acme/lego` v5.6.0 executables verified against upstream
/// `lego_5.6.0_checksums.txt` and `docker.io/goacme/lego:v5.6.0@sha256:6f1583fc026a455ea46a7a5858c576105bb69c2e2de69db391cab7f14750350c`.
pub(crate) const PINNED_TARGETS: &[BuiltinTargetSpec] = &[
    BuiltinTargetSpec {
        os: "linux",
        arch: "x86_64",
        upstream_platform: "linux/amd64",
        size: 41_623_712,
        sha256: "9ba1a7f9ed951e6a506cf7581fcb0373ee3ae9b6fd58d4572a08400c1e905ba8",
        format: BinaryFormat::Elf64Le { e_machine: 62 },
    },
    BuiltinTargetSpec {
        os: "linux",
        arch: "aarch64",
        upstream_platform: "linux/arm64",
        size: 38_600_864,
        sha256: "815ffd1f3ebd65bfbb9cd790bfd18f9cca91f48d8aec3a08732b3fc3b39056d7",
        format: BinaryFormat::Elf64Le { e_machine: 183 },
    },
    BuiltinTargetSpec {
        os: "macos",
        arch: "aarch64",
        upstream_platform: "darwin/arm64",
        size: 40_208_562,
        sha256: "c313fcf379d2245678317cf5e2d52f7b08d84cf63e1173172270f18ab3316b13",
        format: BinaryFormat::MachO64Le {
            cpu_type: 0x0100_000c,
        },
    },
    BuiltinTargetSpec {
        os: "macos",
        arch: "x86_64",
        upstream_platform: "darwin/amd64",
        size: 43_002_272,
        sha256: "2e251cf07a14f2ce52e93e9c5159a5bac5c11c49163f6ae90bac7d5fa67fdcf6",
        format: BinaryFormat::MachO64Le {
            cpu_type: 0x0100_0007,
        },
    },
    BuiltinTargetSpec {
        os: "freebsd",
        arch: "x86_64",
        upstream_platform: "freebsd/amd64",
        size: 41_541_802,
        sha256: "c3f7c848e5921c31157a382535579ea2b35c10d0b0a71e2a582664bbeff62798",
        format: BinaryFormat::Elf64Le { e_machine: 62 },
    },
    BuiltinTargetSpec {
        os: "freebsd",
        arch: "aarch64",
        upstream_platform: "freebsd/arm64",
        size: 38_535_338,
        sha256: "aeb466d40832d290e9b6b95f9ae3fd250c822549039d9c99c840dd13e9eb9224",
        format: BinaryFormat::Elf64Le { e_machine: 183 },
    },
];

pub(crate) fn target_spec_for(os: &str, arch: &str) -> Option<&'static BuiltinTargetSpec> {
    PINNED_TARGETS
        .iter()
        .find(|spec| spec.os == os && spec.arch == arch)
}

pub(crate) struct Invocation<'a> {
    pub workspace: &'a Path,
    pub storage: &'a Path,
    pub email: &'a str,
    pub staging: bool,
    pub domains: &'a [String],
    pub cloudflare_api_token: &'a str,
}

pub(crate) trait IssuerRunner: Send + Sync {
    fn version(&self) -> &str;
    fn verify_ready(&self) -> BoxFuture<'_, Result<(), ExecutionFailure>>;
    fn run<'a>(
        &'a self,
        invocation: Invocation<'a>,
    ) -> BoxFuture<'a, Result<bool, ExecutionFailure>>;
}

#[derive(Clone)]
pub(crate) struct ConfiguredRunner(pub(crate) Arc<dyn IssuerRunner>);

pub(crate) fn runner(state: &AppState) -> Arc<dyn IssuerRunner> {
    Arc::clone(
        &state
            .domain_states
            .get_or_init(|| ConfiguredRunner(Arc::new(BuiltinLegoRunner::default())))
            .0,
    )
}

pub(crate) struct BuiltinLegoRunner {
    path: PathBuf,
    version: &'static str,
}

impl Default for BuiltinLegoRunner {
    fn default() -> Self {
        Self {
            path: PathBuf::from(BUILTIN_LEGO_PATH),
            version: BUILTIN_LEGO_VERSION,
        }
    }
}

impl IssuerRunner for BuiltinLegoRunner {
    fn version(&self) -> &str {
        self.version
    }

    fn verify_ready(&self) -> BoxFuture<'_, Result<(), ExecutionFailure>> {
        Box::pin(async move {
            let spec = target_spec_for(std::env::consts::OS, std::env::consts::ARCH)
                .ok_or_else(|| ExecutionFailure::before("unsupported_issuer_platform"))?;
            verify_builtin_binary(&self.path, spec, self.version, true).await
        })
    }

    fn run<'a>(
        &'a self,
        invocation: Invocation<'a>,
    ) -> BoxFuture<'a, Result<bool, ExecutionFailure>> {
        Box::pin(async move {
            let mut command = build_lego_command(&self.path, &invocation);
            let mut process = command
                .spawn()
                .map_err(|_| ExecutionFailure::before("issuer_spawn_failed"))?;
            let status = match tokio::time::timeout(Duration::from_secs(600), process.wait()).await
            {
                Ok(status) => status.map_err(|_| ExecutionFailure::after("issuer_wait_failed"))?,
                Err(_) => {
                    process
                        .kill()
                        .await
                        .map_err(|_| ExecutionFailure::after("issuer_stop_unconfirmed"))?;
                    process
                        .wait()
                        .await
                        .map_err(|_| ExecutionFailure::after("issuer_stop_unconfirmed"))?;
                    return Err(ExecutionFailure::after(
                        "issuer_timed_out_dns_cleanup_required",
                    ));
                }
            };
            Ok(status.success())
        })
    }
}

pub(crate) fn build_lego_command(
    binary_path: &Path,
    invocation: &Invocation<'_>,
) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(binary_path);
    command
        .arg("run")
        .arg(format!("--path={}", invocation.storage.display()))
        .arg(format!("--email={}", invocation.email))
        .arg("--dns=cloudflare")
        .arg("--key-type=ec256")
        .arg("--accept-tos");
    if invocation.staging {
        command.arg("--server=https://acme-staging-v02.api.letsencrypt.org/directory");
    } else {
        command.arg("--server=https://acme-v02.api.letsencrypt.org/directory");
    }
    for domain in invocation.domains {
        command.arg(format!("--domains={domain}"));
    }
    command
        .current_dir(invocation.workspace)
        .env_clear()
        .env("CF_DNS_API_TOKEN", invocation.cloudflare_api_token)
        .env("CLOUDFLARE_DNS_API_TOKEN", invocation.cloudflare_api_token)
        .env("CLOUDFLARE_POLLING_INTERVAL", "5")
        .env("CLOUDFLARE_PROPAGATION_TIMEOUT", "120")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

pub(crate) fn check_binary_header(
    header: &[u8],
    expected: BinaryFormat,
) -> Result<(), &'static str> {
    match expected {
        BinaryFormat::Elf64Le { e_machine } => {
            if header.starts_with(b"\x7fELF") {
                if header.len() < 20 {
                    return Err("issuer_builtin_binary_invalid");
                }
                let class = header[4];
                let endian = header[5];
                let file_type = u16::from_le_bytes([header[16], header[17]]);
                let machine = u16::from_le_bytes([header[18], header[19]]);
                if class != 2 || endian != 1 || machine != e_machine {
                    return Err("issuer_binary_architecture_mismatch");
                }
                if !matches!(file_type, 2 | 3) {
                    return Err("issuer_builtin_binary_invalid");
                }
                Ok(())
            } else if header.len() >= 4
                && matches!(
                    u32::from_le_bytes([header[0], header[1], header[2], header[3]]),
                    0xcffa_edfe | 0xcefa_edfe | 0xfeed_facf | 0xfeed_face
                )
            {
                Err("issuer_binary_architecture_mismatch")
            } else {
                Err("issuer_builtin_binary_invalid")
            }
        }
        BinaryFormat::MachO64Le { cpu_type } => {
            if header.starts_with(b"\x7fELF") {
                return Err("issuer_binary_architecture_mismatch");
            }
            if header.len() < 16 {
                return Err("issuer_builtin_binary_invalid");
            }
            let magic = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
            if matches!(magic, 0xcefa_edfe | 0xfeed_facf | 0xfeed_face) {
                return Err("issuer_binary_architecture_mismatch");
            }
            if magic != 0xcffa_edfe {
                return Err("issuer_builtin_binary_invalid");
            }
            let actual_cpu = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
            if actual_cpu != cpu_type {
                return Err("issuer_binary_architecture_mismatch");
            }
            let file_type = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
            if file_type != 2 {
                return Err("issuer_builtin_binary_invalid");
            }
            Ok(())
        }
    }
}

pub(crate) async fn verify_builtin_binary(
    path: &Path,
    spec: &BuiltinTargetSpec,
    expected_version: &str,
    require_root_owner: bool,
) -> Result<(), ExecutionFailure> {
    if !path.is_absolute() {
        return Err(ExecutionFailure::before("issuer_builtin_path_invalid"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| ExecutionFailure::before("issuer_builtin_path_invalid"))?;
    for component in path.components() {
        if matches!(component, Component::CurDir | Component::ParentDir) {
            return Err(ExecutionFailure::before("issuer_builtin_path_invalid"));
        }
    }
    let mut prefix = PathBuf::new();
    for component in parent.components() {
        prefix.push(component.as_os_str());
        let metadata = match tokio::fs::symlink_metadata(&prefix).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ExecutionFailure::before("issuer_builtin_binary_missing"));
            }
            Err(_) => return Err(ExecutionFailure::before("issuer_builtin_path_invalid")),
        };
        if !metadata.is_dir() || metadata.is_symlink() {
            return Err(ExecutionFailure::before("issuer_builtin_path_invalid"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.mode() & 0o022 != 0 || (require_root_owner && metadata.uid() != 0) {
                return Err(ExecutionFailure::before(
                    "issuer_builtin_permissions_invalid",
                ));
            }
        }
    }
    let path_meta = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ExecutionFailure::before("issuer_builtin_binary_missing"));
        }
        Err(_) => return Err(ExecutionFailure::before("issuer_builtin_binary_invalid")),
    };
    if !path_meta.is_file() || path_meta.is_symlink() {
        return Err(ExecutionFailure::before("issuer_builtin_binary_invalid"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mode = path_meta.mode();
        if mode & 0o222 != 0
            || mode & 0o111 == 0
            || mode & 0o6000 != 0
            || (require_root_owner && path_meta.uid() != 0)
        {
            return Err(ExecutionFailure::before(
                "issuer_builtin_permissions_invalid",
            ));
        }
    }
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| ExecutionFailure::before("issuer_builtin_binary_invalid"))?;
    let opened_meta = file
        .metadata()
        .await
        .map_err(|_| ExecutionFailure::before("issuer_builtin_binary_invalid"))?;
    if !opened_meta.is_file() {
        return Err(ExecutionFailure::before("issuer_builtin_binary_invalid"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (opened_meta.dev(), opened_meta.ino()) != (path_meta.dev(), path_meta.ino()) {
            return Err(ExecutionFailure::before("issuer_builtin_binary_invalid"));
        }
    }
    let mut hasher = Sha256::new();
    let mut header = Vec::with_capacity(24);
    let mut header_checked = false;
    let mut total = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let count = file
            .read(&mut buffer)
            .await
            .map_err(|_| ExecutionFailure::before("issuer_builtin_binary_invalid"))?;
        if count == 0 {
            break;
        }
        if header.len() < 24 {
            let needed = (24 - header.len()).min(count);
            header.extend_from_slice(&buffer[..needed]);
        }
        if !header_checked && header.len() >= 20 {
            check_binary_header(&header, spec.format).map_err(ExecutionFailure::before)?;
            header_checked = true;
        }
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| ExecutionFailure::before("issuer_builtin_binary_invalid"))?;
        if total > spec.size {
            return Err(ExecutionFailure::before("issuer_builtin_binary_invalid"));
        }
        hasher.update(&buffer[..count]);
    }
    if !header_checked {
        check_binary_header(&header, spec.format).map_err(ExecutionFailure::before)?;
    }
    if total != spec.size || format!("{:x}", hasher.finalize()) != spec.sha256 {
        return Err(ExecutionFailure::before("issuer_builtin_binary_invalid"));
    }
    verify_version_identity(path, expected_version, spec.upstream_platform).await
}

pub(crate) async fn verify_version_identity(
    path: &Path,
    expected_version: &str,
    expected_platform: &str,
) -> Result<(), ExecutionFailure> {
    let mut child = tokio::process::Command::new(path)
        .arg("--version")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| ExecutionFailure::before("issuer_tool_identity_invalid"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ExecutionFailure::before("issuer_tool_identity_invalid"))?;
    let probe = async {
        let mut output = Vec::with_capacity(128);
        let mut limited = stdout.take(256);
        limited
            .read_to_end(&mut output)
            .await
            .map_err(|_| "issuer_tool_identity_invalid")?;
        let status = child
            .wait()
            .await
            .map_err(|_| "issuer_tool_identity_invalid")?;
        if !status.success() {
            return Err("issuer_tool_identity_invalid");
        }
        let text = std::str::from_utf8(&output).map_err(|_| "issuer_tool_identity_invalid")?;
        let expected = format!("lego version {expected_version} {expected_platform}");
        if text.trim() != expected {
            return Err("issuer_tool_identity_invalid");
        }
        Ok(())
    };
    tokio::time::timeout(Duration::from_secs(5), probe)
        .await
        .map_err(|_| ExecutionFailure::before("issuer_tool_identity_invalid"))?
        .map_err(ExecutionFailure::before)
}

pub(super) async fn execute(
    state: &AppState,
    job: Uuid,
    certificate_id: Uuid,
    plan: &Plan,
    plan_revision: i64,
    actor: i64,
) -> Result<Value, ExecutionFailure> {
    if !cfg!(unix) {
        return Err(ExecutionFailure::before("acme_executor_requires_unix"));
    }
    let document = documents::load(state, certificate_id)
        .await
        .map_err(|error| {
            ExecutionFailure::from_pre_api(error, "certificate_configuration_missing")
        })?;
    let job_scope = sqlx::query(
        "SELECT certificate_revision,domain_snapshot FROM network_acme_jobs WHERE id=$1",
    )
    .bind(job)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| ExecutionFailure::from_pre_api(error.into(), "issuance_scope_changed"))?;
    if document.revision != job_scope.get::<i64, _>("certificate_revision") {
        return Err(ExecutionFailure::before("issuance_scope_changed"));
    }
    crate::control_center::require_actor_capability(state, actor, "dns:write")
        .await
        .map_err(|error| ExecutionFailure::from_pre_api(error, "permission_denied"))?;
    crate::control_center::require_actor_capability(state, actor, "network:write")
        .await
        .map_err(|error| ExecutionFailure::from_pre_api(error, "permission_denied"))?;
    let config = documents::configuration(&document).map_err(|error| {
        ExecutionFailure::from_pre_api(error, "certificate_configuration_missing")
    })?;
    for server in config.servers() {
        crate::control_center::require_actor_server(state, actor, server, "network:write")
            .await
            .map_err(|error| ExecutionFailure::from_pre_api(error, "permission_denied"))?;
    }
    let Configuration::Certificate {
        renewal: super::models::RenewalPolicy::Dns01 { ddns_rule_id, .. },
        ..
    } = &config
    else {
        return Err(ExecutionFailure::before(
            "issuer_dns01_maintenance_owner_missing",
        ));
    };
    let dns_server: i64 = sqlx::query_scalar("SELECT server_id FROM ddns_rules WHERE id=$1")
        .bind(ddns_rule_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| ExecutionFailure::from_pre_api(error.into(), "resource_not_found"))?
        .ok_or_else(|| ExecutionFailure::before("resource_not_found"))?;
    crate::control_center::require_actor_server(state, actor, dns_server, "dns:write")
        .await
        .map_err(|error| ExecutionFailure::from_pre_api(error, "permission_denied"))?;
    let Configuration::Certificate { domain_ids, .. } = documents::configuration(&document)
        .map_err(|error| {
            ExecutionFailure::from_pre_api(error, "certificate_configuration_missing")
        })?
    else {
        return Err(ExecutionFailure::before(
            "certificate_configuration_missing",
        ));
    };
    let current = sqlx::query(
        "SELECT revision,account_secret_ref FROM network_acme_plans WHERE certificate_id=$1",
    )
    .bind(certificate_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| ExecutionFailure::from_pre_api(error.into(), "issuance_plan_changed"))?;
    if current.get::<i64, _>("revision") != plan_revision {
        return Err(ExecutionFailure::before("issuance_plan_changed"));
    }
    if !plan.terms_accepted {
        return Err(ExecutionFailure::before("issuer_terms_not_accepted"));
    }
    if !plan.license_accepted {
        return Err(ExecutionFailure::before("issuer_license_not_accepted"));
    }
    if !apsp_panel_host::text::Text::bytes(254).accepts(&plan.email) || !plan.email.contains('@') {
        return Err(ExecutionFailure::before("issuer_email_invalid"));
    }
    let runner = runner(state);
    let expected_version = runner.version().to_owned();
    if plan.tool_version != expected_version {
        return Err(ExecutionFailure {
            code: "issuer_tool_version_mismatch".into(),
            before_execution: true,
            detail: Some(version_mismatch_remediation(
                &plan.tool_version,
                &expected_version,
            )),
            extra: Some(json!({
                "requested_tool_version": plan.tool_version,
                "builtin_tool_version": expected_version,
            })),
        });
    }
    let mut domains = Vec::new();
    for id in domain_ids {
        let domain = documents::load(state, id)
            .await
            .map_err(|error| ExecutionFailure::from_pre_api(error, "certificate_domain_missing"))?;
        let snapshot: Value = job_scope.get("domain_snapshot");
        if !snapshot.as_array().is_some_and(|values| {
            values.iter().any(|value| {
                value["id"].as_str() == Some(id.to_string().as_str())
                    && value["revision"].as_i64() == Some(domain.revision)
            })
        }) {
            return Err(ExecutionFailure::before("issuance_scope_changed"));
        }
        let domain_config = documents::configuration(&domain)
            .map_err(|error| ExecutionFailure::from_pre_api(error, "certificate_domain_missing"))?;
        for server in domain_config.servers() {
            crate::control_center::require_actor_server(state, actor, server, "network:write")
                .await
                .map_err(|error| ExecutionFailure::from_pre_api(error, "permission_denied"))?;
        }
        let Configuration::Domain { name, .. } = domain_config else {
            return Err(ExecutionFailure::before("certificate_domain_missing"));
        };
        domains.push(name);
    }
    if domains.is_empty() {
        return Err(ExecutionFailure::before("certificate_domain_missing"));
    }
    let credentials = crate::control_center::credentials::resolve_reference(
        state,
        plan.credential_id,
        "dns",
        "acme-issuer",
    )
    .await
    .map_err(|error| ExecutionFailure::from_pre_api(error, "issuer_credential_unavailable"))?;
    if credentials
        .get("provider")
        .is_some_and(|value| value.as_str() != Some("cloudflare"))
    {
        return Err(ExecutionFailure::before("issuer_dns_provider_unsupported"));
    }
    let token = credentials["api_token"]
        .as_str()
        .filter(|value| !value.is_empty() && !value.contains(['\0', '\n', '\r']))
        .ok_or_else(|| ExecutionFailure::before("cloudflare_dns_token_missing"))?;
    runner.verify_ready().await?;
    let root = state.config.data_dir.join("network-acme-work");
    secure_directory(&root)
        .await
        .map_err(|_| ExecutionFailure::before("issuer_work_directory_unavailable"))?;
    let workspace = root.join(job.to_string());
    tokio::fs::create_dir(&workspace)
        .await
        .map_err(|_| ExecutionFailure::before("issuer_work_directory_unavailable"))?;
    private_permissions(&workspace, 0o700)
        .await
        .map_err(|_| ExecutionFailure::before("issuer_work_directory_unavailable"))?;
    let cleanup = Cleanup(workspace.clone());
    let mut result = execute_in(
        state,
        runner.as_ref(),
        job,
        certificate_id,
        document.revision,
        plan,
        &expected_version,
        &domains,
        token,
        current.get("account_secret_ref"),
        &workspace,
    )
    .await;
    // This directory is unique to this job and contains only its transient private material.
    // Keep encrypted account/key references and the public certificate version in the database.
    if tokio::fs::remove_dir_all(&workspace).await.is_err() {
        return Err(ExecutionFailure::after(
            "issuer_private_workspace_cleanup_failed",
        ));
    }
    drop(cleanup);
    if let Ok(value) = &mut result {
        value["temporary_material_cleanup"] = json!("confirmed");
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn execute_in(
    state: &AppState,
    runner: &dyn IssuerRunner,
    job: Uuid,
    certificate: Uuid,
    revision: i64,
    plan: &Plan,
    actual_version: &str,
    domains: &[String],
    token: &str,
    account: Option<Uuid>,
    workspace: &Path,
) -> Result<Value, ExecutionFailure> {
    let storage = workspace.join("storage");
    secure_directory(&storage)
        .await
        .map_err(|_| ExecutionFailure::before("issuer_work_directory_unavailable"))?;
    let mut restored_accounts = None;
    if let Some(id) = account {
        let value = crate::control_center::credentials::resolve_reference(
            state,
            id,
            "certificate",
            "acme-account",
        )
        .await
        .map_err(|error| {
            ExecutionFailure::from_pre_api(error, "issuer_account_material_invalid")
        })?;
        let files: BTreeMap<String, String> =
            serde_json::from_value(value["lego_account_files"].clone())
                .map_err(|_| ExecutionFailure::before("issuer_account_material_invalid"))?;
        for (path, content) in &files {
            if !safe_account_path(path) || content.len() > 65536 {
                return Err(ExecutionFailure::before("issuer_account_material_invalid"));
            }
            let destination = storage.join(path);
            let parent = destination
                .parent()
                .ok_or_else(|| ExecutionFailure::before("issuer_account_path_invalid"))?;
            secure_directory(parent)
                .await
                .map_err(|_| ExecutionFailure::before("issuer_account_path_invalid"))?;
            secure_write(&destination, content.as_bytes(), 0o600)
                .await
                .map_err(|_| ExecutionFailure::before("issuer_account_material_invalid"))?;
        }
        restored_accounts = Some(files);
    }
    let succeeded = runner
        .run(Invocation {
            workspace,
            storage: &storage,
            email: &plan.email,
            staging: plan.staging,
            domains,
            cloudflare_api_token: token,
        })
        .await?;
    let account_files = pack_accounts(&storage)
        .await
        .map_err(|_| ExecutionFailure::after("issuer_account_backup_failed"))?;
    if !account_files.is_empty() && restored_accounts.as_ref() != Some(&account_files) {
        let account_ref = crate::control_center::credentials::store_generated(
            &state.pool,
            &format!("ACME account {certificate}"),
            "certificate",
            json!({"lego_account_files": account_files}),
        )
        .await
        .map_err(|_| ExecutionFailure::after("issuer_account_backup_failed"))?;
        sqlx::query("UPDATE network_acme_plans SET account_secret_ref=$2 WHERE certificate_id=$1")
            .bind(certificate)
            .bind(account_ref)
            .execute(&state.pool)
            .await
            .map_err(|_| ExecutionFailure::after("issuer_account_backup_failed"))?;
    }
    if !succeeded {
        return Err(ExecutionFailure::after(
            "issuer_failed_dns_or_acme_validation",
        ));
    }
    let (pem, key) = result_files(&storage, domains)
        .await
        .map_err(|_| ExecutionFailure::after("issued_certificate_material_invalid"))?;
    let parsed =
        x509::parse(&pem).map_err(|_| ExecutionFailure::after("issued_certificate_invalid"))?;
    if parsed.not_after <= apsp_protocol::now_timestamp() {
        return Err(ExecutionFailure::after("issued_certificate_expired"));
    }
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
    let chain = CertificateDer::pem_slice_iter(pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ExecutionFailure::after("issued_chain_invalid"))?;
    let private = PrivateKeyDer::from_pem_slice(key.as_bytes())
        .map_err(|_| ExecutionFailure::after("issued_key_invalid"))?;
    rustls::sign::CertifiedKey::from_der(chain, private, &rustls::crypto::ring::default_provider())
        .map_err(|_| ExecutionFailure::after("issued_certificate_key_mismatch"))?;
    let private_ref = crate::control_center::credentials::store_generated(
        &state.pool,
        &format!("Certificate key {certificate} {actual_version}"),
        "certificate",
        json!({"private_key": key, "fingerprint": parsed.fingerprint}),
    )
    .await
    .map_err(|_| ExecutionFailure::after("issuer_private_key_store_failed"))?;
    let version = Uuid::new_v4();
    let now = apsp_protocol::now_timestamp();
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|_| ExecutionFailure::after("issuer_version_save_failed"))?;
    let actual = documents::lock_revision(&mut tx, certificate)
        .await
        .map_err(|_| ExecutionFailure::after("issuer_version_save_failed"))?;
    let version_revision: i64 = sqlx::query_scalar(
        "SELECT COALESCE(max(revision),0)+1 FROM network_certificate_versions WHERE certificate_id=$1",
    )
    .bind(certificate)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| ExecutionFailure::after("issuer_version_save_failed"))?;
    sqlx::query(
        "INSERT INTO network_certificate_versions(id,certificate_id,revision,public_chain,fingerprint,not_before,not_after,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(version)
    .bind(certificate)
    .bind(version_revision)
    .bind(pem)
    .bind(&parsed.fingerprint)
    .bind(parsed.not_before)
    .bind(parsed.not_after)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|_| ExecutionFailure::after("issuer_version_save_failed"))?;
    let selected = actual == revision;
    if selected {
        documents::activate_version(&mut tx, certificate, version, now)
            .await
            .map_err(|_| ExecutionFailure::after("issuer_version_save_failed"))?;
    }
    sqlx::query("UPDATE network_acme_jobs SET key_secret_ref=$2,version_id=$3 WHERE id=$1")
        .bind(job)
        .bind(private_ref)
        .bind(version)
        .execute(&mut *tx)
        .await
        .map_err(|_| ExecutionFailure::after("issuer_version_save_failed"))?;
    tx.commit()
        .await
        .map_err(|_| ExecutionFailure::after("issuer_version_save_failed"))?;
    Ok(json!({
        "issued": true,
        "saved": true,
        "deployed": false,
        "handshake_verified": false,
        "version_id": version,
        "private_key_reference": private_ref,
        "fingerprint": parsed.fingerprint,
        "not_after": parsed.not_after,
        "active_version_selected": selected,
        "tool": "lego",
        "tool_version": actual_version,
        "tool_supply": "panel_builtin_verified_binary",
        "staging": plan.staging,
        "temporary_material_cleanup": "required_before_completion",
    }))
}

async fn result_files(storage: &Path, domains: &[String]) -> anyhow::Result<(String, String)> {
    let mut files = tokio::fs::read_dir(storage.join("certificates")).await?;
    while let Some(file) = files.next_entry().await? {
        let path = file.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !name.ends_with(".crt") || name.ends_with(".issuer.crt") {
            continue;
        }
        let pem = ordinary_text(&path, 65536).await?;
        let cert = x509::parse(&pem)?;
        if domains
            .iter()
            .all(|domain| x509::covers(&cert.names, domain))
        {
            return Ok((
                pem,
                ordinary_text(&path.with_extension("key"), 16384).await?,
            ));
        }
    }
    anyhow::bail!("certificate chain not found")
}

async fn pack_accounts(storage: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    let mut stack = vec![storage.join("accounts")];
    let mut total = 0usize;
    while let Some(directory) = stack.pop() {
        if !directory.try_exists()? {
            continue;
        }
        let mut entries = tokio::fs::read_dir(directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let metadata = tokio::fs::symlink_metadata(&path).await?;
            ensure!(!metadata.is_symlink(), "account material cannot be symlink");
            if metadata.is_dir() {
                ensure!(stack.len() < 32, "account tree too deep");
                stack.push(path);
                continue;
            }
            let name = path
                .strip_prefix(storage)?
                .to_str()
                .context("account path is not UTF8")?
                .to_owned();
            ensure!(safe_account_path(&name), "unsafe account path");
            let content = ordinary_text(&path, 65536).await?;
            total += content.len();
            ensure!(
                total <= 48000 && result.len() < 64,
                "account material exceeds credential limit"
            );
            result.insert(name, content);
        }
    }
    ensure!(
        serde_json::to_vec(&json!({"lego_account_files": result}))?.len() <= 65536,
        "encoded account material exceeds credential limit"
    );
    Ok(result)
}

pub(crate) fn safe_account_path(path: &str) -> bool {
    path.starts_with("accounts/")
        && path.len() <= 1024
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && !path.contains(['\\', '\0', '\n', '\r'])
}

async fn ordinary_text(path: &Path, max: u64) -> anyhow::Result<String> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    ensure!(
        metadata.is_file() && !metadata.is_symlink() && metadata.len() <= max,
        "material is not a bounded ordinary file"
    );
    Ok(tokio::fs::read_to_string(path).await?)
}

async fn secure_directory(path: &Path) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(path).await?;
    let metadata = tokio::fs::symlink_metadata(path).await?;
    ensure!(
        metadata.is_dir() && !metadata.is_symlink(),
        "private directory is not ordinary"
    );
    private_permissions(path, 0o700).await
}

async fn secure_write(path: &Path, bytes: &[u8], mode: u32) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut options = tokio::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        options.mode(mode);
    }
    let mut file = options.open(path).await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    Ok(())
}

async fn private_permissions(path: &Path, mode: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await?;
    }
    Ok(())
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

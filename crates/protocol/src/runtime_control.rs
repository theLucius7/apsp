use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const RUNTIME_CHECKPOINT_CAPABILITY: &str = "runtime:checkpoint-v1";
pub const RUNTIME_RECOVERY_BARRIER_CAPABILITY: &str = "runtime:barrier-v1";
pub const RUNTIME_PATH_PROBE_CAPABILITY: &str = "runtime:path-probe-v1";
pub const RUNTIME_CONTROL_MAX_TTL_SECS: i64 = 600;
pub const RUNTIME_CHECKPOINT_REQUEST_KIND: &str = "runtime.checkpoint.request";
pub const RUNTIME_BARRIER_REQUEST_KIND: &str = "runtime.barrier.request";
pub const RUNTIME_PATH_PROBE_REQUEST_KIND: &str = "runtime.path_probe.request";

/// Selects one probe already authorized by the signed, applied bundle.
/// Neither credentials nor an arbitrary target are accepted from a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePathProbeRequest {
    pub request_id: Uuid,
    pub expected: RuntimeCheckpoint,
    pub probe_id: Uuid,
    pub expires_at: i64,
}

impl RuntimePathProbeRequest {
    pub fn valid(&self) -> bool {
        !self.request_id.is_nil()
            && !self.probe_id.is_nil()
            && self.expected.valid()
            && self.expected.healthy
            && self.expires_at > 0
    }
    pub fn valid_at(&self, now: i64) -> bool {
        self.valid()
            && deadline_valid(self.expires_at, now)
            && self.expires_at.saturating_sub(now) <= 120
    }
    pub fn digest(&self) -> serde_json::Result<String> {
        request_digest(RUNTIME_PATH_PROBE_REQUEST_KIND, self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePathProbeResult {
    pub request_id: Uuid,
    pub request_digest: String,
    pub observed: Option<RuntimeCheckpoint>,
    pub probe_id: Uuid,
    pub elapsed_ms: Option<u64>,
    pub success: bool,
    pub error: Option<String>,
}

impl RuntimePathProbeResult {
    pub fn valid(&self) -> bool {
        !self.request_id.is_nil()
            && !self.probe_id.is_nil()
            && hash(&self.request_digest)
            && self.observed.as_ref().is_none_or(RuntimeCheckpoint::valid)
            && result_fields_valid(self.success, &self.observed, &self.error)
            && self
                .elapsed_ms
                .is_none_or(|elapsed| elapsed > 0 && elapsed <= 5000)
            && (self.success == self.elapsed_ms.is_some())
    }
}

fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn runtime_module_valid(module: &str) -> bool {
    !module.is_empty()
        && module.len() <= 64
        && module
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

fn printable(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= maximum
        && value.bytes().all(|byte| (b' '..=b'~').contains(&byte))
}

fn deadline_valid(expires_at: i64, now: i64) -> bool {
    expires_at > now && expires_at.saturating_sub(now) <= RUNTIME_CONTROL_MAX_TTL_SECS
}

fn request_digest<T: Serialize>(kind: &str, request: &T) -> serde_json::Result<String> {
    let mut digest = Sha256::new();
    digest.update(b"apsp-runtime-control-v1\0");
    digest.update(kind.as_bytes());
    digest.update([0]);
    digest.update(serde_json::to_vec(request)?);
    Ok(format!("{:x}", digest.finalize()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBinding {
    pub deployment_id: Uuid,
    pub module: String,
    pub revision: u64,
    pub bundle_sha256: String,
    pub binding_digest: String,
}

impl RuntimeBinding {
    pub fn new(deployment_id: Uuid, module: String, revision: u64, bundle_sha256: String) -> Self {
        let mut value = Self {
            deployment_id,
            module,
            revision,
            bundle_sha256,
            binding_digest: String::new(),
        };
        value.binding_digest = value.computed_binding_digest();
        value
    }

    /// Identifies this deployment; it is neither a signature nor path verification.
    pub fn computed_binding_digest(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"apsp-runtime-binding-v1\0");
        digest.update(self.deployment_id.as_bytes());
        digest.update((self.module.len() as u64).to_be_bytes());
        digest.update(self.module.as_bytes());
        digest.update(self.revision.to_be_bytes());
        digest.update(self.bundle_sha256.as_bytes());
        format!("{:x}", digest.finalize())
    }

    pub fn valid(&self) -> bool {
        !self.deployment_id.is_nil()
            && runtime_module_valid(&self.module)
            && self.revision > 0
            && self.revision <= i64::MAX as u64
            && hash(&self.bundle_sha256)
            && hash(&self.binding_digest)
            && self.binding_digest == self.computed_binding_digest()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCheckpoint {
    pub binding: RuntimeBinding,
    pub activation_id: Uuid,
    pub instance_id: String,
    pub healthy: bool,
}

impl RuntimeCheckpoint {
    pub fn valid(&self) -> bool {
        self.binding.valid() && !self.activation_id.is_nil() && printable(&self.instance_id, 256)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCheckpointRequest {
    pub request_id: Uuid,
    pub expected: RuntimeBinding,
    pub expires_at: i64,
}

impl RuntimeCheckpointRequest {
    pub fn valid(&self) -> bool {
        !self.request_id.is_nil() && self.expected.valid() && self.expires_at > 0
    }

    pub fn valid_at(&self, now: i64) -> bool {
        self.valid() && deadline_valid(self.expires_at, now)
    }

    pub fn digest(&self) -> serde_json::Result<String> {
        request_digest(RUNTIME_CHECKPOINT_REQUEST_KIND, self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCheckpointResult {
    pub request_id: Uuid,
    pub request_digest: String,
    pub observed: Option<RuntimeCheckpoint>,
    pub success: bool,
    pub error: Option<String>,
}

impl RuntimeCheckpointResult {
    pub fn valid(&self) -> bool {
        !self.request_id.is_nil()
            && hash(&self.request_digest)
            && self.observed.as_ref().is_none_or(RuntimeCheckpoint::valid)
            && result_fields_valid(self.success, &self.observed, &self.error)
    }
}

fn result_fields_valid(
    success: bool,
    observed: &Option<RuntimeCheckpoint>,
    error: &Option<String>,
) -> bool {
    if success {
        observed.as_ref().is_some_and(|value| value.healthy) && error.is_none()
    } else {
        error.as_ref().is_some_and(|value| {
            !value.trim().is_empty() && value.len() <= 1024 && !value.chars().any(char::is_control)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeControlAck {
    pub request_id: Uuid,
    pub request_digest: String,
}

impl RuntimeControlAck {
    pub fn valid(&self) -> bool {
        !self.request_id.is_nil() && hash(&self.request_digest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeRecoveryBarrierRequest {
    pub request_id: Uuid,
    pub expected: RuntimeCheckpoint,
    pub minimum_revision: u64,
    pub expires_at: i64,
}

impl RuntimeRecoveryBarrierRequest {
    pub fn valid(&self) -> bool {
        !self.request_id.is_nil()
            && self.expected.valid()
            && self.expected.healthy
            && self.minimum_revision > 0
            && self.minimum_revision <= self.expected.binding.revision
            && self.expires_at > 0
    }

    pub fn valid_at(&self, now: i64) -> bool {
        self.valid() && deadline_valid(self.expires_at, now)
    }

    pub fn digest(&self) -> serde_json::Result<String> {
        request_digest(RUNTIME_BARRIER_REQUEST_KIND, self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeRecoveryBarrierResult {
    pub request_id: Uuid,
    pub request_digest: String,
    pub observed: Option<RuntimeCheckpoint>,
    pub minimum_revision: Option<u64>,
    pub pending_intents_clear: bool,
    pub success: bool,
    pub error: Option<String>,
}

impl RuntimeRecoveryBarrierResult {
    pub fn valid(&self) -> bool {
        !self.request_id.is_nil()
            && hash(&self.request_digest)
            && self.observed.as_ref().is_none_or(RuntimeCheckpoint::valid)
            && result_fields_valid(self.success, &self.observed, &self.error)
            && self
                .minimum_revision
                .is_none_or(|value| value > 0 && value <= i64::MAX as u64)
            && (!self.success
                || (self.pending_intents_clear
                    && self.minimum_revision.is_some_and(|floor| {
                        self.observed
                            .as_ref()
                            .is_some_and(|observed| floor <= observed.binding.revision)
                    })))
    }
}

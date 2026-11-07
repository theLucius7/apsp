use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const RUNTIME_OPERATIONS_CAPABILITY: &str = "runtime:operations:v1";
pub const RUNTIME_SESSIONS_CAPABILITY: &str = "runtime:sessions:v1";
pub const RUNTIME_LOG_LIMIT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOperation {
    Inspect,
    Restart,
    RetryDeployment,
    InspectSessions,
    CloseSessions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOperationRequest {
    pub id: Uuid,
    pub module: String,
    pub operation: RuntimeOperation,
    pub expected_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_filter: Option<RuntimeSessionFilter>,
    pub requested_at: i64,
    pub expires_at: i64,
}

impl RuntimeOperationRequest {
    pub fn valid(&self) -> bool {
        !self.module.is_empty()
            && self.module.len() <= 64
            && self
                .module
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            && self.requested_at > 0
            && self.expires_at > self.requested_at
            && self.expires_at.saturating_sub(self.requested_at) <= 600
            && match self.operation {
                RuntimeOperation::InspectSessions | RuntimeOperation::CloseSessions => {
                    self.session_filter
                        .as_ref()
                        .is_some_and(RuntimeSessionFilter::valid)
                        && self
                            .expected_revision
                            .is_some_and(|rev| rev > 0 && rev <= i64::MAX as u64)
                }
                RuntimeOperation::Inspect => {
                    self.expected_revision.is_none() && self.session_filter.is_none()
                }
                _ => {
                    self.session_filter.is_none()
                        && self
                            .expected_revision
                            .is_some_and(|rev| rev > 0 && rev <= i64::MAX as u64)
                }
            }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOperationError {
    Expired,
    Interrupted,
    ModuleUnavailable,
    TargetChanged,
    Retiring,
    ManifestUnavailable,
    OperationFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeServiceState {
    Active,
    Inactive,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeLogLevel {
    Error,
    Warning,
    Info,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeLogKind {
    Started,
    Stopped,
    ConnectionFailed,
    ConfigurationFailed,
    CertificateFailed,
    Other,
}

/// No free-form runtime text crosses the transport boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLogEntry {
    pub timestamp: Option<i64>,
    pub level: RuntimeLogLevel,
    pub kind: RuntimeLogKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSnapshot {
    pub observed_at: i64,
    pub applied_revision: Option<u64>,
    pub service: RuntimeServiceState,
    pub healthy: Option<bool>,
    pub logs_available: bool,
    pub logs_service_events: bool,
    pub logs_truncated: bool,
    pub logs: Vec<RuntimeLogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOperationResult {
    pub id: Uuid,
    pub module: String,
    pub operation: RuntimeOperation,
    pub finished_at: i64,
    pub error: Option<RuntimeOperationError>,
    pub snapshot: Option<RuntimeSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions: Option<RuntimeConnections>,
}

impl RuntimeOperationResult {
    pub fn valid(&self) -> bool {
        self.module.len() <= 64
            && self.finished_at > 0
            && self.snapshot.as_ref().is_none_or(|snapshot| {
                snapshot.logs.len() <= RUNTIME_LOG_LIMIT
                    && snapshot.observed_at > 0
                    && snapshot.observed_at <= self.finished_at
                    && (snapshot.logs_available || snapshot.logs.is_empty())
                    && snapshot.logs.iter().all(|entry| {
                        entry.timestamp.is_none_or(|at| {
                            at > 0 && at <= snapshot.observed_at.saturating_add(60)
                        })
                    })
            })
            && self
                .sessions
                .as_ref()
                .is_none_or(|report| report.valid() && report.observed_at <= self.finished_at)
            && (!matches!(
                self.operation,
                RuntimeOperation::InspectSessions | RuntimeOperation::CloseSessions
            ) || (self.snapshot.is_none() && (self.error.is_some() || self.sessions.is_some())))
            && (self.error.is_some() || self.snapshot.is_some() || self.sessions.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_operations_reject_shell_parameters_and_unbounded_expiry() {
        let request = RuntimeOperationRequest {
            id: Uuid::new_v4(),
            module: "demo".into(),
            operation: RuntimeOperation::Restart,
            expected_revision: Some(2),
            session_filter: None,
            requested_at: 1,
            expires_at: 601,
        };
        assert!(request.valid());
        let mut invalid = request.clone();
        invalid.expires_at += 1;
        assert!(!invalid.valid());
        invalid = request.clone();
        invalid.module = "../../service".into();
        assert!(!invalid.valid());
        let mut value = serde_json::to_value(&request).unwrap();
        value["command"] = "arbitrary".into();
        assert!(serde_json::from_value::<RuntimeOperationRequest>(value).is_err());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSessionFilter {
    pub principals: Vec<String>,
    pub ids: Vec<String>,
}
impl RuntimeSessionFilter {
    pub fn valid(&self) -> bool {
        !self.principals.is_empty()
            && self.principals.len() <= 256
            && self.ids.len() <= 256
            && self.principals.iter().chain(&self.ids).all(|s| {
                !s.is_empty()
                    && s.len() <= 128
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
            })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConnection {
    pub id: String,
    pub principal: String,
    pub source: std::net::SocketAddr,
    pub network: String,
    pub upload: u64,
    pub download: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConnections {
    pub observed_at: i64,
    pub revision: u64,
    pub truncated: bool,
    pub closed: u32,
    pub connections: Vec<RuntimeConnection>,
}
impl RuntimeConnections {
    pub fn valid(&self) -> bool {
        self.observed_at > 0
            && self.revision > 0
            && self.connections.len() <= 256
            && self.closed <= 256
            && self.connections.iter().all(|c| {
                !c.id.is_empty()
                    && c.id.len() <= 128
                    && c.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    && !c.principal.is_empty()
                    && c.principal.len() <= 128
                    && c.principal
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                    && matches!(c.network.as_str(), "tcp" | "udp")
            })
    }
}

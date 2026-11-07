use super::Reconciler;
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::{Plan, ServiceLogLine};
use apsp_protocol::{
    RUNTIME_LOG_LIMIT, RuntimeLogEntry, RuntimeLogKind, RuntimeLogLevel, RuntimeServiceState,
    RuntimeSnapshot, now_timestamp,
};
use uuid::Uuid;

impl Reconciler {
    pub(crate) async fn connection_operation(
        &self,
        request: &apsp_protocol::RuntimeOperationRequest,
    ) -> Result<apsp_protocol::RuntimeConnections> {
        let _guard = self.gate.lock().await;
        let previous = self.previous()?.context("no applied runtime")?;
        ensure!(
            Some(previous.spec.revision) == request.expected_revision,
            "applied revision changed"
        );
        self.verify_applied_runtime(&previous).await?;
        let filter = request
            .session_filter
            .as_ref()
            .context("missing connection filter")?;
        ensure!(filter.valid(), "invalid connection filter");
        let source = self
            .adapter
            .session_source()
            .context("session control unsupported")?;
        let connections = self
            .bounded(source.connections(&previous, &filter.principals))
            .await?;
        let truncated = connections.len() > 256;
        let mut closed = 0;
        if request.operation == apsp_protocol::RuntimeOperation::CloseSessions {
            let ids: Vec<String> = connections
                .iter()
                .filter(|entry| filter.ids.is_empty() || filter.ids.contains(&entry.id))
                .take(256)
                .map(|entry| entry.id.clone())
                .collect();
            closed = self.bounded(source.close_sessions(&previous, &ids)).await? as u32;
        }
        let connections = if request.operation == apsp_protocol::RuntimeOperation::CloseSessions {
            self.bounded(source.connections(&previous, &filter.principals))
                .await?
        } else {
            connections
        };
        Ok(apsp_protocol::RuntimeConnections {
            observed_at: now_timestamp(),
            revision: previous.spec.revision,
            truncated: truncated || connections.len() > 256,
            closed,
            connections: connections
                .into_iter()
                .take(256)
                .map(|c| apsp_protocol::RuntimeConnection {
                    id: c.id,
                    principal: c.principal,
                    source: c.source,
                    network: c.network,
                    upload: c.upload,
                    download: c.download,
                })
                .collect(),
        })
    }

    pub(crate) async fn restart_current(
        &self,
        target: &apsp_protocol::ModuleManifest,
        client: &crate::artifacts::PanelClient,
    ) -> Result<()> {
        let _guard = self.gate.lock().await;
        self.recover_locked().await?;
        let descriptor = client.runtime_descriptor(
            &target.artifact,
            &target.kernel_version,
            self.adapter.as_ref(),
        )?;
        client.verify_artifact(&target.artifact, &target.kernel_version, &descriptor)?;
        let previous = self.previous()?.context("no applied runtime")?;
        ensure!(
            previous.spec.revision == target.config_rev
                && previous.spec.config_hash == target.bundle_sha256
                && previous.spec.kernel_version == target.kernel_version,
            "desired runtime changed"
        );
        self.restart_locked(previous).await
    }

    /// Restart the applied revision through the same durable rollback path as deployment.
    pub async fn restart_applied(&self, revision: u64) -> Result<()> {
        let _guard = self.gate.lock().await;
        self.recover_locked().await?;
        let previous = self.previous()?.context("no applied runtime")?;
        ensure!(
            previous.spec.revision == revision,
            "applied revision changed"
        );
        self.restart_locked(previous).await
    }

    async fn restart_locked(&self, previous: apsp_adapter_sdk::Prepared) -> Result<()> {
        self.verify_applied_runtime(&previous).await?;
        let prepared = self
            .bounded(
                self.adapter
                    .prepare(previous.spec.clone(), self.privileged.as_ref()),
            )
            .await?;
        self.apply_plan_locked(Some(previous), prepared, Plan::Restart, Uuid::new_v4())
            .await
    }

    pub async fn runtime_snapshot(&self) -> Result<RuntimeSnapshot> {
        let _guard = self.gate.lock().await;
        let descriptor = self.adapter.describe();
        let previous = self.previous()?;
        let service = match self
            .bounded(self.services.is_active(&descriptor.service_unit))
            .await
        {
            Ok(true) => RuntimeServiceState::Active,
            Ok(false) => RuntimeServiceState::Inactive,
            Err(_) => RuntimeServiceState::Unknown,
        };
        let healthy = if let Some(previous) = &previous {
            let observed: Result<()> = async {
                self.require_revision_floor(previous.spec.revision)?;
                self.verify_applied_runtime(previous).await?;
                if self.services.supports_runtime_checkpoint()
                    && let Some(activation) = self.saved_activation()?
                {
                    self.validate_activation(previous, &activation).await?;
                }
                ensure!(self.runtime_health(previous).await?, "runtime is unhealthy");
                Ok(())
            }
            .await;
            Some(observed.is_ok())
        } else {
            None
        };
        let logs = self
            .bounded(self.services.recent_logs(&descriptor.service_unit))
            .await;
        let observed_at = now_timestamp();
        let (logs_available, logs_truncated, logs_service_events, logs) = match logs {
            Ok(logs) => {
                let truncated = logs.truncated || logs.lines.len() > RUNTIME_LOG_LIMIT;
                let mut entries: Vec<_> = logs
                    .lines
                    .iter()
                    .rev()
                    .take(RUNTIME_LOG_LIMIT)
                    .map(|line| redact_line(line, observed_at))
                    .collect();
                entries.reverse();
                (true, truncated, logs.service_events, entries)
            }
            Err(_) => (false, false, false, Vec::new()),
        };
        Ok(RuntimeSnapshot {
            observed_at,
            applied_revision: previous.map(|runtime| runtime.spec.revision),
            service,
            healthy,
            logs_available,
            logs_truncated,
            logs_service_events,
            logs,
        })
    }
}

fn redact_line(line: &ServiceLogLine, now: i64) -> RuntimeLogEntry {
    // Classification is intentionally lossy: unknown text and all dynamic values stay local.
    let text = line
        .text
        .chars()
        .take(4096)
        .collect::<String>()
        .to_ascii_lowercase();
    let level = match line.priority {
        Some(0..=3) => RuntimeLogLevel::Error,
        Some(4) => RuntimeLogLevel::Warning,
        Some(5..=7) => RuntimeLogLevel::Info,
        _ if text.contains("error") || text.contains("fatal") || text.contains("failed") => {
            RuntimeLogLevel::Error
        }
        _ if text.contains("warn") => RuntimeLogLevel::Warning,
        _ if text.contains("info") => RuntimeLogLevel::Info,
        _ => RuntimeLogLevel::Unknown,
    };
    let failed = text.contains("fail") || text.contains("error");
    let kind = if failed && (text.contains("certificate") || text.contains("tls")) {
        RuntimeLogKind::CertificateFailed
    } else if failed && text.contains("config") {
        RuntimeLogKind::ConfigurationFailed
    } else if failed && (text.contains("connect") || text.contains("dial")) {
        RuntimeLogKind::ConnectionFailed
    } else if text.contains("started") {
        RuntimeLogKind::Started
    } else if text.contains("stopped") || text.contains("shutting down") {
        RuntimeLogKind::Stopped
    } else {
        RuntimeLogKind::Other
    };
    RuntimeLogEntry {
        timestamp: line.timestamp.filter(|at| *at > 0 && *at <= now + 60),
        level,
        kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_logs_never_retain_secrets_or_unknown_fields() {
        let line = ServiceLogLine {
            text:
                "ERROR connect failed password=secret-token tls={private_key:secret-key} 192.0.2.1"
                    .into(),
            timestamp: Some(i64::MAX),
            priority: None,
        };
        let entry = redact_line(&line, 100);
        let wire = serde_json::to_string(&entry).unwrap();
        for secret in ["secret", "192.0.2.1", "password", "private_key"] {
            assert!(!wire.contains(secret));
        }
        assert_eq!(entry.timestamp, None);
        assert_eq!(entry.level, RuntimeLogLevel::Error);
        assert_eq!(entry.kind, RuntimeLogKind::CertificateFailed);
    }
}

use super::*;

impl DiagnosticWorker {
    pub(super) async fn observe(&self, checkpoint: &Checkpoint) -> Result<()> {
        let Checkpoint::Started { spec, service, .. } = checkpoint else {
            anyhow::bail!("only a started diagnostic can be observed");
        };
        let id = super::finalization::validate_saved_target(&self.config, spec, service)?;
        if self
            .read::<bool>(&format!("diagnostics:done:{id}"))?
            .unwrap_or(false)
            || !matches!(self.active()?, Some(Checkpoint::Started { spec: current, .. }) if current.id == spec.id)
        {
            return Ok(());
        }
        let result = self.observe_execution(checkpoint).await;
        if let Err(error) = &result
            && let Err(storage) = self.record_cleanup_error(id, error)
        {
            tracing::warn!(%id, %storage, "pending diagnostic cleanup error could not be recorded; ownership is retained");
        }
        result
    }

    async fn observe_execution(&self, checkpoint: &Checkpoint) -> Result<()> {
        let Checkpoint::Started {
            spec,
            service,
            started_at,
            tool,
            start_error,
            expires_at,
            protection_stop_reason,
            terminal_update,
            ..
        } = checkpoint
        else {
            anyhow::bail!("only a started diagnostic can be observed");
        };
        let id = Uuid::parse_str(&spec.id)?;
        if self.cancellation_requested(id)? {
            return Ok(());
        }
        if terminal_update.is_some() {
            return self.finish_cleanup(checkpoint).await;
        }
        let execution_deadline = started_at
            .saturating_add(u64::from(spec.timeout_secs))
            .saturating_add(61);
        let absolute_deadline = expires_at
            .map(|deadline| u64::try_from(deadline).unwrap_or(0))
            .unwrap_or(u64::MAX);
        let deadline = execution_deadline.min(absolute_deadline);
        let deadline_reason = if absolute_deadline <= execution_deadline {
            "diagnostic reached its absolute deadline"
        } else {
            "diagnostic exceeded its execution deadline"
        };
        let (status, stop_reason) = self
            .status_with_memory_protection(
                &service.unit,
                protection_stop_reason.as_deref(),
                deadline,
                deadline_reason,
            )
            .await?;
        let mut stopping = checkpoint.clone();
        if let Err(error) = self.capture_environment(checkpoint) {
            tracing::warn!(%id, %error, "execution environment capture failed; resource protection continues");
        }
        if protection_stop_reason.is_none()
            && let Some(reason) = &stop_reason
        {
            if let Checkpoint::Started {
                protection_stop_reason,
                ..
            } = &mut stopping
            {
                *protection_stop_reason = Some(reason.clone());
            }
            match self.save_if_owned(&stopping) {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(error) => {
                    // Storage failure must not prevent the memory protection action.
                    let stopped = self.bounded(self.services.stop(&service.unit)).await;
                    tracing::warn!(%id, result=?stopped, "diagnostic stopped despite protection checkpoint storage failure");
                    return Err(error).context(
                        "cannot persist diagnostic protection reason; ownership retained",
                    );
                }
            }
        }
        if status == Some(JobStatus::Running) && stop_reason.is_none() {
            // Reading a damaged output filesystem must not delay resource protection.
            if let Err(error) =
                tokio::time::timeout(Duration::from_secs(2), self.capture_sections(spec, tool))
                    .await
                    .context("diagnostic chapter capture timed out")
                    .and_then(|result| result)
            {
                tracing::warn!(%id, %error, "diagnostic chapter capture failed; saved chapters are retained");
            }
            return Ok(());
        }
        if status == Some(JobStatus::Running) || stop_reason.is_some() {
            self.bounded(self.services.stop(&service.unit))
                .await
                .context("诊断保护停止未确认，将保留任务并继续重试")?;
            ensure!(
                !self.bounded(self.services.is_active(&service.unit)).await?
                    && self
                        .bounded(self.services.job_status(&service.unit))
                        .await?
                        != JobStatus::Running,
                "诊断仍有活动进程，保护停止未确认，将继续重试"
            );
        }
        if let Err(error) =
            tokio::time::timeout(Duration::from_secs(2), self.capture_sections(spec, tool))
                .await
                .context("final diagnostic chapter capture timed out")
                .and_then(|result| result)
        {
            tracing::warn!(%id, %error, "final diagnostic chapter capture failed; saved chapters are retained");
        }
        let collected = if let Some(adapter) = self.adapters.get(tool) {
            self.bounded(adapter.collect(spec)).await
        } else {
            Err(anyhow::anyhow!("diagnostic tool is no longer registered"))
        };
        let report = collected
            .as_ref()
            .ok()
            .and_then(|output| output.as_ref())
            .filter(|output| !output.text.trim().is_empty() && output.text.len() <= MAX_REPORT)
            .map(|output| DiagnosticReport {
                text: output.text.clone(),
                report_url: output.report_url.clone(),
            });
        let update = if let Some(reason) = stop_reason {
            failure(id, reason, report)
        } else {
            match status.context("diagnostic status is unavailable")? {
                JobStatus::Succeeded if report.is_some() => DiagnosticUpdate {
                    id,
                    status: DiagnosticStatus::Succeeded,
                    report,
                    error: None,
                },
                JobStatus::Succeeded => failure(
                    id,
                    collected
                        .err()
                        .map(|error| format!("collect diagnostic: {error}"))
                        .unwrap_or_else(|| "diagnostic completed without a valid report".into()),
                    report,
                ),
                JobStatus::Failed { error } => failure(id, error, report),
                JobStatus::Missing => failure(
                    id,
                    format!(
                        "diagnostic service is missing after restart or an uncertain start; task was not repeated{}",
                        start_error
                            .as_ref()
                            .map(|error| format!("; start error: {error}"))
                            .unwrap_or_default()
                    ),
                    report,
                ),
                JobStatus::Running => failure(id, deadline_reason.into(), report),
            }
        };
        if self.cancellation_requested(id)? {
            return Ok(());
        }
        if let Checkpoint::Started {
            terminal_update,
            cleanup_error,
            ..
        } = &mut stopping
        {
            *terminal_update = Some(Box::new(update));
            *cleanup_error = None;
        }
        // Freeze the observed result before cleanup changes the systemd state.
        // Recovery retries only cleanup and does not reread a damaged report.
        if !self.save_if_owned(&stopping)? {
            return Ok(());
        }
        self.finish_cleanup(&stopping).await
    }
}

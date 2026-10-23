use super::*;

impl DiagnosticWorker {
    pub(in crate::transport::diagnostics) fn cancellation_requested(
        &self,
        id: Uuid,
    ) -> Result<bool> {
        self.cancellations
            .as_ref()
            .map_or(Ok(false), |control| control.requested(id))
    }

    pub(in crate::transport::diagnostics) async fn process_cancellations(&self) -> Result<()> {
        let Some(control) = &self.cancellations else {
            return Ok(());
        };
        for request in control.pending()? {
            // A historical cancellation must not block protection of the device's
            // current owner. Its durable intent waits until that owner is clean.
            if let Some(Checkpoint::Started { spec, .. }) = self.active()?
                && spec.id != request.job.id.to_string()
            {
                continue;
            }
            // Finish only the cleanup already in flight; retirement owns the rest.
            if self.retiring() {
                break;
            }
            let result = self.cancel_known_job(&request).await;
            let (confirmed, report, error) = match result {
                Ok(report) => (true, report, None),
                Err(error) => (
                    false,
                    None,
                    Some(format!("取消尚未确认，将继续重试：{error:#}")),
                ),
            };
            control.save_result(DiagnosticCancelResult {
                server_id: request.server_id,
                id: request.job.id,
                plugin: request.job.plugin,
                confirmed,
                report,
                error,
            })?;
        }
        Ok(())
    }

    async fn cancel_known_job(
        &self,
        request: &DiagnosticCancelRequest,
    ) -> Result<Option<DiagnosticReport>> {
        let job = &request.job;
        let adapter = self.adapters.get(&job.plugin);
        let unit = format!("apsp-diagnostic-{}.service", job.id);
        let directory = self
            .config
            .runtime_root
            .join("diagnostics")
            .join(job.id.to_string());
        let mut frozen_report = None;
        let mut had_frozen_outcome = false;
        let spec = match self.active()? {
            Some(Checkpoint::Started {
                spec,
                service,
                plugin,
                terminal_update,
                ..
            }) if spec.id == job.id.to_string() => {
                ensure!(
                    plugin == job.plugin
                        && spec.version == job.version
                        && service.unit == unit
                        && service.working_directory == directory
                        && spec.job_dir == directory,
                    "saved cancellation target differs from the known task"
                );
                super::super::finalization::validate_saved_target(&self.config, &spec, &service)?;
                had_frozen_outcome = terminal_update.is_some();
                frozen_report = terminal_update.and_then(|update| update.report);
                spec
            }
            Some(Checkpoint::Preparing(known)) if known.id == job.id => {
                ensure!(
                    known == *job,
                    "cancel request differs from the preparing task"
                );
                self.cancellation_spec(
                    job,
                    directory.clone(),
                    adapter
                        .context("cancel plugin is no longer registered")?
                        .as_ref(),
                )
            }
            _ => self.cancellation_spec(
                job,
                directory.clone(),
                adapter
                    .context("cancel plugin is no longer registered")?
                    .as_ref(),
            ),
        };
        // The durable request binds this generated unit to the authenticated task.
        // Never accept a unit name from the wire, and never start a unit to cancel it.
        self.bounded(super::super::finalization::stop_and_confirm(
            self.services.as_ref(),
            &unit,
            &directory,
        ))
        .await?;
        // The service has stopped, so collect its final chapter snapshots before
        // removing the active checkpoint. Failures do not erase saved chapters.
        if let Err(error) = tokio::time::timeout(
            Duration::from_secs(2),
            self.capture_sections(&spec, &job.plugin),
        )
        .await
        .context("cancelled diagnostic chapter capture timed out")
        .and_then(|result| result)
        {
            tracing::warn!(%error, "cancelled diagnostic chapters will retain their saved snapshots");
        }
        let pending: Vec<DiagnosticUpdate> = self.read(OUTBOX)?.unwrap_or_default();
        let existing = pending
            .into_iter()
            .find(|update| update.id == job.id)
            .and_then(|update| update.report);
        if had_frozen_outcome {
            return Ok(frozen_report.or(existing));
        }
        if let Some(report) = existing {
            return Ok(Some(report));
        }
        let Some(adapter) = adapter else {
            return Ok(None);
        };
        let collected = self.bounded(adapter.collect(&spec)).await;
        let report = collected
            .ok()
            .flatten()
            .filter(|output| !output.text.trim().is_empty() && output.text.len() <= MAX_REPORT)
            .map(|output| DiagnosticReport {
                text: output.text,
                report_url: output.report_url,
            });
        Ok(report)
    }

    fn cancellation_spec(
        &self,
        job: &DiagnosticJob,
        directory: std::path::PathBuf,
        adapter: &dyn DiagnosticAdapter,
    ) -> DiagnosticSpec {
        let descriptor = adapter.describe();
        DiagnosticSpec {
            id: job.id.to_string(),
            version: job.version.clone(),
            binary_path: self
                .config
                .install_root
                .join(descriptor.plugin_name)
                .join(&job.version)
                .join(descriptor.binary_name),
            job_dir: directory,
            timeout_secs: u32::try_from(job.timeout_secs).unwrap_or(1),
            options: job.options.clone(),
        }
    }
}

use super::Reconciler;
use crate::state::runtime_control::ControlResult;
use anyhow::{Context, Result, ensure};
use apsp_protocol::{RuntimePathProbeRequest, RuntimePathProbeResult};
use std::time::Duration;
use tokio::time::{Instant, timeout_at};

impl Reconciler {
    /// Called under the same gate as apply, observation and recovery barriers.
    pub(super) async fn execute_path_probe(
        &self,
        request: &RuntimePathProbeRequest,
        digest: String,
    ) -> Result<ControlResult> {
        let remaining = request
            .expires_at
            .saturating_sub(self.runtime_control_now()?);
        let deadline = Instant::now() + Duration::from_secs(remaining.clamp(0, 15) as u64);
        let attempt = timeout_at(deadline, async {
            ensure!(
                request.valid_at(self.runtime_control_now()?),
                "runtime verification request expired"
            );
            ensure!(
                self.supports_runtime_probe(),
                "runtime verification is unsupported"
            );
            let module = &request.expected.binding.module;
            ensure!(
                module == &self.adapter.describe().module,
                "runtime verification module mismatch"
            );
            ensure!(
                self.state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state poisoned"))?
                    .pending_intents()?
                    .iter()
                    .all(|intent| &intent.module != module),
                "unfinished runtime intent prevents verification"
            );
            let before = self.observe_checkpoint(&request.expected.binding).await?;
            ensure!(
                before == request.expected,
                "runtime activation changed before verification"
            );
            let runtime = self.previous()?.context("no applied runtime")?;
            let id = request.probe_id.to_string();
            let measurement = timeout_at(
                deadline.min(Instant::now() + Duration::from_secs(5)),
                self.adapter.runtime_probe(&runtime, &id),
            )
            .await
            .context("runtime verification timed out")??;
            ensure!(
                measurement.elapsed_ms > 0 && measurement.elapsed_ms <= 5000,
                "runtime verification measurement exceeds budget"
            );
            let after = self.observe_checkpoint(&request.expected.binding).await?;
            ensure!(
                after == before,
                "runtime activation changed during verification"
            );
            ensure!(
                request.valid_at(self.runtime_control_now()?) && Instant::now() < deadline,
                "runtime verification deadline elapsed"
            );
            Ok::<_, anyhow::Error>((after, measurement.elapsed_ms))
        })
        .await;
        let (observed, elapsed_ms, error) = match attempt {
            Ok(Ok((observed, elapsed))) => (Some(observed), Some(elapsed), None),
            // Adapter and native errors are not public transport data.
            _ => (
                None,
                None,
                Some("runtime path verification failed or exceeded its deadline".into()),
            ),
        };
        let result = ControlResult::Probe(RuntimePathProbeResult {
            request_id: request.request_id,
            request_digest: digest,
            observed,
            probe_id: request.probe_id,
            elapsed_ms,
            success: error.is_none(),
            error,
        });
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("state poisoned"))?
            .finish_runtime_control(&result, None)?;
        Ok(result)
    }
}

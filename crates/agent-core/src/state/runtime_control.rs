use super::State;
use anyhow::{Context, Result, ensure};
use apsp_protocol::{
    Envelope, RuntimeCheckpointRequest, RuntimeCheckpointResult, RuntimeControlAck,
    RuntimePathProbeRequest, RuntimePathProbeResult, RuntimeRecoveryBarrierRequest,
    RuntimeRecoveryBarrierResult,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MAX_ROWS: u64 = 4096;
const MAX_PENDING: u64 = 256;
const MAX_ROW_BYTES: usize = 16 * 1024;
const MAX_BATCH_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "payload")]
pub enum ControlRequest {
    Checkpoint(RuntimeCheckpointRequest),
    Barrier(RuntimeRecoveryBarrierRequest),
    Probe(RuntimePathProbeRequest),
}

impl ControlRequest {
    pub fn id(&self) -> Uuid {
        match self {
            Self::Checkpoint(v) => v.request_id,
            Self::Barrier(v) => v.request_id,
            Self::Probe(v) => v.request_id,
        }
    }
    pub fn module(&self) -> &str {
        match self {
            Self::Checkpoint(v) => &v.expected.module,
            Self::Barrier(v) => &v.expected.binding.module,
            Self::Probe(v) => &v.expected.binding.module,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Checkpoint(_) => "checkpoint",
            Self::Barrier(_) => "barrier",
            Self::Probe(_) => "probe",
        }
    }
    pub fn valid(&self) -> bool {
        match self {
            Self::Checkpoint(v) => v.valid(),
            Self::Barrier(v) => v.valid(),
            Self::Probe(v) => v.valid(),
        }
    }
    pub fn valid_at(&self, now: i64) -> bool {
        match self {
            Self::Checkpoint(v) => v.valid_at(now),
            Self::Barrier(v) => v.valid_at(now),
            Self::Probe(v) => v.valid_at(now),
        }
    }
    pub fn digest(&self) -> Result<String> {
        Ok(match self {
            Self::Checkpoint(v) => v.digest()?,
            Self::Barrier(v) => v.digest()?,
            Self::Probe(v) => v.digest()?,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "payload")]
pub enum ControlResult {
    Checkpoint(RuntimeCheckpointResult),
    Barrier(RuntimeRecoveryBarrierResult),
    Probe(RuntimePathProbeResult),
}

impl ControlResult {
    pub fn id(&self) -> Uuid {
        match self {
            Self::Checkpoint(v) => v.request_id,
            Self::Barrier(v) => v.request_id,
            Self::Probe(v) => v.request_id,
        }
    }
    pub fn digest(&self) -> &str {
        match self {
            Self::Checkpoint(v) => &v.request_digest,
            Self::Barrier(v) => &v.request_digest,
            Self::Probe(v) => &v.request_digest,
        }
    }
    fn kind(&self) -> &'static str {
        match self {
            Self::Checkpoint(_) => "checkpoint",
            Self::Barrier(_) => "barrier",
            Self::Probe(_) => "probe",
        }
    }
    fn valid(&self) -> bool {
        match self {
            Self::Checkpoint(v) => v.valid(),
            Self::Barrier(v) => v.valid(),
            Self::Probe(v) => v.valid(),
        }
    }
    pub fn envelope(&self) -> Result<Envelope> {
        Ok(match self {
            Self::Checkpoint(v) => Envelope::new("runtime.checkpoint.result", v)?,
            Self::Barrier(v) => Envelope::new("runtime.barrier.result", v)?,
            Self::Probe(v) => Envelope::new("runtime.path_probe.result", v)?,
        })
    }
}

impl State {
    /// Completed identifiers are retained as bounded tombstones, never reused or TTL-evicted.
    pub fn enqueue_runtime_control(
        &mut self,
        request: &ControlRequest,
    ) -> Result<Option<ControlResult>> {
        ensure!(request.valid(), "invalid runtime control request");
        let digest = request.digest()?;
        let raw = serde_json::to_string(request)?;
        ensure!(
            raw.len() <= MAX_ROW_BYTES,
            "runtime request exceeds byte limit"
        );
        let existing: Option<(String, String, String, Option<String>)> = self
            .connection
            .query_row(
                "SELECT digest, kind, request, result FROM runtime_control WHERE request_id = ?1",
                [request.id().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if let Some((saved_digest, kind, saved, result)) = existing {
            ensure!(
                saved_digest == digest && kind == request.kind() && saved == raw,
                "runtime request identifier was already bound to different work"
            );
            return result
                .map(|raw| serde_json::from_str(&raw).context("decode runtime result"))
                .transpose();
        }
        let (rows, pending): (u64, u64) = self.connection.query_row(
            "SELECT COUNT(*), COALESCE(SUM(acknowledged = 0), 0) FROM runtime_control",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        ensure!(
            rows < MAX_ROWS && pending < MAX_PENDING,
            "runtime control journal is full; unacknowledged work retained"
        );
        self.connection.execute(
            "INSERT INTO runtime_control(request_id, digest, kind, module, request) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![request.id().to_string(), digest, request.kind(), request.module(), raw],
        )?;
        Ok(None)
    }

    pub fn pending_runtime_controls(&self) -> Result<Vec<ControlRequest>> {
        let mut statement = self.connection.prepare(
            "SELECT request FROM runtime_control WHERE result IS NULL ORDER BY rowid LIMIT 16",
        )?;
        let mut output = Vec::new();
        let mut bytes = 0;
        for raw in statement.query_map([], |row| row.get::<_, String>(0))? {
            let raw = raw?;
            ensure!(
                raw.len() <= MAX_ROW_BYTES,
                "saved runtime request exceeds byte limit"
            );
            bytes += raw.len();
            if bytes > MAX_BATCH_BYTES {
                break;
            }
            output.push(serde_json::from_str(&raw)?);
        }
        Ok(output)
    }

    pub fn pending_runtime_results(&self) -> Result<Vec<ControlResult>> {
        Ok(self
            .runtime_results_after(0)?
            .into_iter()
            .map(|(_, result)| result)
            .collect())
    }

    pub(crate) fn runtime_results_after(&self, cursor: i64) -> Result<Vec<(i64, ControlResult)>> {
        let mut statement = self.connection.prepare(
            "SELECT rowid, result FROM runtime_control WHERE result IS NOT NULL AND acknowledged = 0 AND rowid > ?1 ORDER BY rowid LIMIT 16")?;
        let mut output = Vec::new();
        let mut bytes = 0;
        for row in statement.query_map([cursor], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })? {
            let (position, raw) = row?;
            ensure!(
                raw.len() <= MAX_ROW_BYTES,
                "saved runtime result exceeds byte limit"
            );
            bytes += raw.len();
            if bytes > MAX_BATCH_BYTES {
                break;
            }
            output.push((position, serde_json::from_str(&raw)?));
        }
        Ok(output)
    }

    pub fn runtime_control_result(
        &self,
        request: &ControlRequest,
    ) -> Result<Option<ControlResult>> {
        let row: Option<(String, String, Option<String>)> = self
            .connection
            .query_row(
                "SELECT digest, kind, result FROM runtime_control WHERE request_id = ?1",
                [request.id().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let (digest, kind, result) = row.context("runtime request has not been persisted")?;
        ensure!(
            digest == request.digest()? && kind == request.kind(),
            "runtime request identity mismatch"
        );
        result
            .map(|raw| serde_json::from_str(&raw).context("decode runtime result"))
            .transpose()
    }

    pub fn runtime_revision_floor(&self, module: &str) -> Result<u64> {
        let floor: Option<String> = self
            .connection
            .query_row(
                "SELECT revision FROM runtime_revision_floors WHERE module = ?1",
                [module],
                |row| row.get(0),
            )
            .optional()?;
        floor
            .map(|value| {
                value
                    .parse::<u64>()
                    .context("invalid persisted recovery floor")
            })
            .transpose()
            .map(|value| value.unwrap_or(0))
    }

    /// The recovery promise and its acknowledgement share one FULL-synchronous commit.
    pub fn finish_runtime_control(
        &mut self,
        result: &ControlResult,
        floor: Option<(&str, u64)>,
    ) -> Result<()> {
        ensure!(result.valid(), "invalid runtime result");
        let raw = serde_json::to_string(result)?;
        ensure!(
            raw.len() <= MAX_ROW_BYTES,
            "runtime result exceeds byte limit"
        );
        let transaction = self.connection.transaction()?;
        let (digest, kind, module, request, saved): (String, String, String, String, Option<String>) = transaction.query_row(
            "SELECT digest, kind, module, request, result FROM runtime_control WHERE request_id = ?1", [result.id().to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )?;
        ensure!(
            digest == result.digest() && kind == result.kind(),
            "runtime result identity mismatch"
        );
        if let Some(saved) = saved {
            ensure!(saved == raw, "runtime result cannot be replaced");
            return Ok(());
        }
        let request: ControlRequest = serde_json::from_str(&request)?;
        match (&request, result) {
            (ControlRequest::Checkpoint(request), ControlResult::Checkpoint(result))
                if result.success =>
            {
                ensure!(
                    result
                        .observed
                        .as_ref()
                        .is_some_and(|observed| observed.binding == request.expected),
                    "checkpoint receipt differs from the persisted request"
                );
            }
            (ControlRequest::Probe(request), ControlResult::Probe(result)) => {
                ensure!(
                    (!result.success || result.observed.as_ref() == Some(&request.expected))
                        && result.probe_id == request.probe_id
                        && floor.is_none(),
                    "probe receipt differs from the persisted request"
                );
            }
            (ControlRequest::Barrier(request), ControlResult::Barrier(result))
                if result.success =>
            {
                ensure!(
                    result.observed.as_ref() == Some(&request.expected)
                        && result.minimum_revision == Some(request.minimum_revision)
                        && floor == Some((module.as_str(), request.minimum_revision)),
                    "barrier receipt or floor differs from the persisted request"
                );
            }
            _ => {}
        }
        if let Some((floor_module, revision)) = floor {
            ensure!(floor_module == module, "recovery floor module mismatch");
            let ControlResult::Barrier(barrier) = result else {
                anyhow::bail!("only barriers may advance the recovery floor");
            };
            ensure!(
                barrier.success && barrier.minimum_revision == Some(revision),
                "unsuccessful barrier cannot change the recovery floor"
            );
            let pending: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM intents WHERE completed = 0 AND module = ?1)",
                [&module],
                |row| row.get(0),
            )?;
            ensure!(
                !pending,
                "unfinished runtime intent prevents a recovery barrier"
            );
            let old: Option<String> = transaction
                .query_row(
                    "SELECT revision FROM runtime_revision_floors WHERE module = ?1",
                    [&module],
                    |row| row.get(0),
                )
                .optional()?;
            ensure!(
                revision >= old.map(|v| v.parse::<u64>()).transpose()?.unwrap_or(0),
                "recovery floor cannot decrease"
            );
            transaction.execute(
                "INSERT INTO runtime_revision_floors(module,revision) VALUES (?1,?2) ON CONFLICT(module) DO UPDATE SET revision=excluded.revision",
                params![module, revision.to_string()],
            )?;
        }
        transaction.execute(
            "UPDATE runtime_control SET result = ?2 WHERE request_id = ?1 AND result IS NULL",
            params![result.id().to_string(), raw],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn acknowledge_runtime_control(
        &mut self,
        kind: &str,
        ack: &RuntimeControlAck,
    ) -> Result<()> {
        ensure!(ack.valid(), "invalid runtime acknowledgement");
        let updated = self.connection.execute(
            "UPDATE runtime_control SET acknowledged = 1 WHERE request_id = ?1 AND digest = ?2 AND kind = ?3 AND result IS NOT NULL",
            params![ack.request_id.to_string(), ack.request_digest, kind],
        )?;
        ensure!(
            updated == 1,
            "runtime acknowledgement does not match a completed request"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests;

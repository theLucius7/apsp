use crate::State;
use anyhow::{Result, ensure};
use apsp_protocol::{TelemetryAck, TelemetrySample};
use rusqlite::params;

const MAX_ROWS: usize = 7200;
const MAX_BYTES: i64 = 64 * 1024 * 1024;
const MAX_AGE_MS: i64 = 2 * 60 * 60 * 1000;

impl State {
    pub fn save_telemetry(&mut self, sample: &TelemetrySample) -> Result<usize> {
        let payload = serde_json::to_string(sample)?;
        ensure!(
            payload.len() <= 128 * 1024,
            "telemetry sample exceeds size limit"
        );
        let floor = self.latest_telemetry_timestamp()?.max(sample.sampled_at);
        let tx = self.connection.transaction()?;
        // Persist the timestamp floor atomically with the sample, and retain it
        // after an ACK removes the outbox so restart cannot move time backwards.
        crate::state::write_json(&tx, "telemetry:last_sampled_at", &floor)?;
        tx.execute(
            "INSERT INTO telemetry_outbox(id,sampled_at,payload) VALUES(?1,?2,?3)",
            params![sample.id.to_string(), sample.sampled_at, payload],
        )?;
        let mut removed = tx.execute(
            "DELETE FROM telemetry_outbox WHERE sampled_at<?1",
            [sample.sampled_at.saturating_sub(MAX_AGE_MS)],
        )?;
        removed += tx.execute("DELETE FROM telemetry_outbox WHERE rowid NOT IN (SELECT rowid FROM telemetry_outbox ORDER BY rowid DESC LIMIT ?1)", [MAX_ROWS as i64])?;
        let mut bytes: i64 = tx.query_row(
            "SELECT COALESCE(SUM(length(payload)),0) FROM telemetry_outbox",
            [],
            |row| row.get(0),
        )?;
        while bytes > MAX_BYTES {
            let (row, length): (i64, i64) = tx.query_row(
                "SELECT rowid,length(payload) FROM telemetry_outbox ORDER BY rowid LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            removed += tx.execute("DELETE FROM telemetry_outbox WHERE rowid=?1", [row])?;
            bytes -= length;
        }
        tx.commit()?;
        Ok(removed)
    }

    pub(crate) fn latest_telemetry_timestamp(&self) -> Result<i64> {
        // The indexed outbox maximum also covers upgrades from older Agents that
        // did not yet persist a timestamp floor.
        let pending: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(sampled_at),0) FROM telemetry_outbox",
            [],
            |row| row.get(0),
        )?;
        Ok(pending.max(
            self.get_json::<i64>("telemetry:last_sampled_at")?
                .unwrap_or(0),
        ))
    }

    pub fn pending_telemetry(&self) -> Result<Vec<TelemetrySample>> {
        let mut statement = self
            .connection
            .prepare("SELECT payload FROM telemetry_outbox ORDER BY sampled_at,rowid LIMIT 64")?;
        let mut samples = Vec::new();
        let mut bytes = 0;
        for row in statement.query_map([], |row| row.get::<_, String>(0))? {
            let row = row?;
            if bytes + row.len() > 900 * 1024 {
                break;
            }
            bytes += row.len();
            samples.push(serde_json::from_str(&row)?);
        }
        Ok(samples)
    }

    pub fn acknowledge_telemetry(&mut self, ack: &TelemetryAck) -> Result<()> {
        ensure!(
            ack.ids.len() <= 64,
            "telemetry acknowledgment exceeds batch size"
        );
        let tx = self.connection.transaction()?;
        for id in &ack.ids {
            tx.execute("DELETE FROM telemetry_outbox WHERE id=?1", [id.to_string()])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn pending_telemetry_count(&self) -> Result<usize> {
        Ok(self
            .connection
            .query_row("SELECT COUNT(*) FROM telemetry_outbox", [], |row| {
                row.get(0)
            })?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apsp_protocol::Metrics;
    use std::path::Path;
    use uuid::Uuid;

    #[test]
    fn telemetry_survives_reopening_and_only_acknowledged_samples_are_removed() -> Result<()> {
        let path = std::env::temp_dir().join(format!("sn-telemetry-{}.db", Uuid::new_v4()));
        let sample = TelemetrySample {
            id: Uuid::new_v4(),
            sampled_at: 1000,
            metrics: Metrics::default(),
        };
        State::open(&path)?.save_telemetry(&sample)?;
        let mut state = State::open(&path)?;
        assert_eq!(state.pending_telemetry()?, vec![sample.clone()]);
        state.acknowledge_telemetry(&TelemetryAck {
            ids: vec![Uuid::new_v4()],
        })?;
        assert_eq!(state.pending_telemetry_count()?, 1);
        state.acknowledge_telemetry(&TelemetryAck {
            ids: vec![sample.id],
        })?;
        assert_eq!(state.pending_telemetry_count()?, 0);
        assert_eq!(state.latest_telemetry_timestamp()?, sample.sampled_at);
        drop(state);
        assert_eq!(
            State::open(&path)?.latest_telemetry_timestamp()?,
            sample.sampled_at
        );
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn retention_and_payload_limits_keep_offline_storage_bounded() -> Result<()> {
        let mut state = State::open(Path::new(":memory:"))?;
        let mut sample = TelemetrySample {
            id: Uuid::new_v4(),
            sampled_at: 1000,
            metrics: Metrics::default(),
        };
        state.save_telemetry(&sample)?;
        sample.id = Uuid::new_v4();
        sample.sampled_at += MAX_AGE_MS + 1;
        assert_eq!(state.save_telemetry(&sample)?, 1);
        sample.id = Uuid::new_v4();
        sample.metrics.extra.insert(
            "oversized".into(),
            serde_json::json!("x".repeat(128 * 1024)),
        );
        assert!(state.save_telemetry(&sample).is_err());
        assert_eq!(state.pending_telemetry_count()?, 1);
        Ok(())
    }
}

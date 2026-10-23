use anyhow::Result;

#[derive(Default)]
pub(crate) struct StorageRetry {
    unavailable: bool,
}

impl StorageRetry {
    pub(crate) fn finish<T>(
        &mut self,
        operation: &'static str,
        result: Result<T>,
    ) -> Result<Option<T>> {
        match result {
            Ok(value) => {
                if self.unavailable {
                    tracing::info!(operation, "state storage recovered");
                }
                self.unavailable = false;
                Ok(Some(value))
            }
            Err(error) if recoverable(&error) => {
                if !self.unavailable {
                    tracing::warn!(%error, operation, "state storage temporarily unavailable; durable data retained for retry");
                }
                self.unavailable = true;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

fn recoverable(error: &anyhow::Error) -> bool {
    matches!(error.downcast_ref::<rusqlite::Error>(), Some(rusqlite::Error::SqliteFailure(error, _))
        if matches!(error.code, rusqlite::ErrorCode::DiskFull | rusqlite::ErrorCode::SystemIoFailure
            | rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::State;
    use apsp_protocol::{Metrics, TelemetrySample, UsageBatch};
    use std::path::Path;
    use uuid::Uuid;

    #[test]
    fn real_sqlite_full_rolls_back_and_retries_without_erasing_durable_samples() -> Result<()> {
        let mut state = State::open(Path::new(":memory:"))?;
        let old = TelemetrySample {
            id: Uuid::new_v4(),
            sampled_at: 1000,
            metrics: Metrics::default(),
        };
        state.save_telemetry(&old)?;
        let pages: i64 = state
            .connection
            .query_row("PRAGMA page_count", [], |row| row.get(0))?;
        state
            .connection
            .execute_batch(&format!("PRAGMA max_page_count={pages}"))?;
        let mut sample = TelemetrySample {
            id: Uuid::new_v4(),
            sampled_at: 2000,
            metrics: Metrics::default(),
        };
        sample.metrics.extra.insert(
            "TEST_ONLY_bounded_payload".into(),
            serde_json::json!("x".repeat(60 * 1024)),
        );
        let mut retry = StorageRetry::default();
        assert!(
            retry
                .finish("TEST_ONLY persist", state.save_telemetry(&sample))?
                .is_none()
        );
        assert_eq!(state.pending_telemetry()?, vec![old.clone()]);
        assert_eq!(state.latest_telemetry_timestamp()?, old.sampled_at);
        state
            .connection
            .execute_batch("PRAGMA max_page_count=100000")?;
        assert!(
            retry
                .finish("TEST_ONLY persist", state.save_telemetry(&sample))?
                .is_some()
        );
        assert_eq!(state.pending_telemetry()?, vec![old, sample]);
        // Structural corruption and protocol validation errors never become transient retries.
        state
            .connection
            .execute("DROP TABLE telemetry_outbox", [])?;
        assert!(
            retry
                .finish("TEST_ONLY invalid structure", state.pending_telemetry())
                .is_err()
        );
        assert!(
            retry
                .finish::<()>(
                    "TEST_ONLY invalid protocol",
                    Err(anyhow::anyhow!("invalid acknowledgment"))
                )
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn full_acknowledged_cleanup_retains_rows_until_storage_recovers() -> Result<()> {
        let mut state = State::open(Path::new(":memory:"))?;
        let batch = UsageBatch {
            epoch: Uuid::new_v4(),
            seq: 1,
            period_start: 10,
            period_end: 20,
            records: vec![],
        };
        state.connection.execute(
            "INSERT INTO usage_outbox(epoch,seq,batch,acknowledged) VALUES(?1,'1',?2,1)",
            rusqlite::params![batch.epoch.to_string(), serde_json::to_string(&batch)?],
        )?;
        state.connection.execute_batch(
            "CREATE TABLE TEST_ONLY_full(payload BLOB);
            CREATE TRIGGER TEST_ONLY_cleanup_full BEFORE DELETE ON usage_outbox
            BEGIN INSERT INTO TEST_ONLY_full VALUES(zeroblob(65536)); END;",
        )?;
        let pages: i64 = state
            .connection
            .query_row("PRAGMA page_count", [], |row| row.get(0))?;
        state
            .connection
            .execute_batch(&format!("PRAGMA max_page_count={pages}"))?;
        let mut retry = StorageRetry::default();
        assert!(
            retry
                .finish("TEST_ONLY cleanup", state.cleanup_acknowledged())?
                .is_none()
        );
        assert_eq!(
            state
                .connection
                .query_row("SELECT count(*) FROM usage_outbox", [], |row| row
                    .get::<_, i64>(0))?,
            1
        );
        state
            .connection
            .execute_batch("PRAGMA max_page_count=100000")?;
        assert!(
            retry
                .finish("TEST_ONLY cleanup", state.cleanup_acknowledged())?
                .is_some()
        );
        assert_eq!(
            state
                .connection
                .query_row("SELECT count(*) FROM usage_outbox", [], |row| row
                    .get::<_, i64>(0))?,
            0
        );
        Ok(())
    }
}

use crate::state::{State, read_json, write_json};
use anyhow::{Context, Result, bail};
use apsp_adapter_sdk::Counter;
use apsp_protocol::{UsageBatch, UsageRecord};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

const LAST_SEQUENCE_KEY: &str = "usage:last_seq";

pub const MAX_PENDING_USAGE_BATCHES: usize = 64;
pub const MAX_PENDING_USAGE_BYTES: usize = 1024 * 1024 - 1;
pub const MAX_USAGE_BATCH_BYTES: usize = 128 * 1024;
const MAX_USAGE_BATCH_RECORDS: usize = 10_000;
// Includes the envelope UUID and the longest possible protocol version/timestamp.
const ENVELOPE_RESERVE_BYTES: usize = 128;
const BATCH_RESERVE_BYTES: usize = 256;

#[derive(Serialize, Deserialize)]
struct UsageClock {
    epoch: Uuid,
    timestamp: i64,
}

impl State {
    /// Atomically advances baselines and persists bounded batches for all deltas.
    /// Returns the first batch; all batches are replayed from the durable outbox.
    pub fn sample_usage(
        &mut self,
        module: &str,
        counters: &[Counter],
        timestamp: i64,
    ) -> Result<Option<UsageBatch>> {
        validate_module(module)?;
        let mut names = BTreeSet::new();
        for counter in counters {
            if counter.stat_name.is_empty()
                || counter.stat_name.len() > 512
                || counter.stat_name.chars().any(char::is_control)
            {
                bail!("invalid counter name");
            }
            if !names.insert(&counter.stat_name) {
                bail!("duplicate counter name in one sample");
            }
        }
        let transaction = self.connection.transaction()?;
        let clock_key = clock_key(module);
        let mut clock: UsageClock = read_json(&transaction, &clock_key)?.unwrap_or(UsageClock {
            epoch: Uuid::new_v4(),
            timestamp,
        });
        if timestamp < clock.timestamp {
            bail!("usage timestamp moved backwards");
        }
        let period_start = clock.timestamp;
        let mut records = Vec::new();
        let mut decreased = false;
        for counter in counters {
            let previous: Option<(String, String)> = transaction.query_row(
                "SELECT uplink, downlink FROM usage_baselines WHERE module = ?1 AND stat_name = ?2",
                params![module, counter.stat_name], |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            let (previous_up, previous_down) = previous
                .map(|(up, down)| -> Result<(u64, u64)> {
                    Ok((
                        up.parse().context("invalid saved uplink counter")?,
                        down.parse().context("invalid saved downlink counter")?,
                    ))
                })
                .transpose()?
                .unwrap_or((0, 0));
            let uplink = delta(counter.uplink, previous_up, &mut decreased);
            let downlink = delta(counter.downlink, previous_down, &mut decreased);
            if uplink != 0 || downlink != 0 {
                records.push(UsageRecord {
                    stat_name: counter.stat_name.clone(),
                    uplink,
                    downlink,
                });
            }
        }
        if decreased {
            clock.epoch = Uuid::new_v4();
            transaction.execute(
                "UPDATE usage_baselines SET epoch = ?2 WHERE module = ?1",
                params![module, clock.epoch.to_string()],
            )?;
        }
        for counter in counters {
            transaction.execute(
                "INSERT INTO usage_baselines (module, stat_name, epoch, uplink, downlink, observed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (module, stat_name) DO UPDATE SET epoch = excluded.epoch, uplink = excluded.uplink, downlink = excluded.downlink, observed_at = excluded.observed_at",
                params![module, counter.stat_name, clock.epoch.to_string(), counter.uplink.to_string(), counter.downlink.to_string(), timestamp],
            )?;
        }
        clock.timestamp = timestamp;
        write_json(&transaction, &clock_key, &clock)?;
        let batch = if records.is_empty() {
            None
        } else {
            records.sort_by(|left, right| left.stat_name.cmp(&right.stat_name));
            let mut seq: u64 = read_json(&transaction, LAST_SEQUENCE_KEY)?.unwrap_or(0);
            let mut first = None;
            let mut chunk = Vec::new();
            let mut bytes = BATCH_RESERVE_BYTES + ENVELOPE_RESERVE_BYTES;
            for record in records {
                let record_bytes = serde_json::to_vec(&record)?.len() + 1;
                if !chunk.is_empty()
                    && (bytes + record_bytes > MAX_USAGE_BATCH_BYTES
                        || chunk.len() == MAX_USAGE_BATCH_RECORDS)
                {
                    let saved = persist_batch(
                        &transaction,
                        clock.epoch,
                        &mut seq,
                        period_start,
                        timestamp,
                        std::mem::take(&mut chunk),
                    )?;
                    first.get_or_insert(saved);
                    bytes = BATCH_RESERVE_BYTES + ENVELOPE_RESERVE_BYTES;
                }
                anyhow::ensure!(
                    bytes + record_bytes <= MAX_USAGE_BATCH_BYTES,
                    "usage record exceeds batch byte limit"
                );
                bytes += record_bytes;
                chunk.push(record);
            }
            let saved = persist_batch(
                &transaction,
                clock.epoch,
                &mut seq,
                period_start,
                timestamp,
                chunk,
            )?;
            first.get_or_insert(saved);
            write_json(&transaction, LAST_SEQUENCE_KEY, &seq)?;
            first
        };
        transaction.commit()?;
        if decreased {
            tracing::warn!(
                module,
                "cumulative usage counter decreased; a new epoch was opened and a missing collection window may exist"
            );
        }
        Ok(batch)
    }

    /// Starts a new cycle only when the caller knows the runtime counters reset.
    pub fn begin_usage_epoch(&mut self, module: &str, timestamp: i64) -> Result<()> {
        validate_module(module)?;
        let transaction = self.connection.transaction()?;
        let key = clock_key(module);
        if let Some(previous) = read_json::<UsageClock>(&transaction, &key)?
            && timestamp < previous.timestamp
        {
            bail!("usage timestamp moved backwards");
        }
        transaction.execute("DELETE FROM usage_baselines WHERE module = ?1", [module])?;
        write_json(
            &transaction,
            &key,
            &UsageClock {
                epoch: Uuid::new_v4(),
                timestamp,
            },
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn pending_usage(&self) -> Result<Vec<UsageBatch>> {
        // The literal byte predicate matches the partial index. Filtering after
        // LIMIT or scanning oversized rows would let a legacy backlog stall replay.
        let mut statement = self.connection.prepare(
            "WITH candidates AS MATERIALIZED (
                SELECT rowid, seq, octet_length(batch) + ?1 AS wire_bytes
                FROM usage_outbox INDEXED BY usage_outbox_pending_order_idx
                WHERE acknowledged = 0 AND octet_length(batch) <= 1048447
                ORDER BY length(seq), seq LIMIT ?2
            ), sized AS (
                SELECT rowid, seq, SUM(wire_bytes) OVER (
                    ORDER BY length(seq), seq ROWS UNBOUNDED PRECEDING
                ) AS total_bytes FROM candidates
            )
            SELECT usage_outbox.batch FROM sized
            JOIN usage_outbox ON usage_outbox.rowid = sized.rowid
            WHERE total_bytes <= ?3 ORDER BY length(sized.seq), sized.seq LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![
                ENVELOPE_RESERVE_BYTES,
                MAX_PENDING_USAGE_BATCHES,
                MAX_PENDING_USAGE_BYTES
            ],
            |row| row.get::<_, String>(0),
        )?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    /// Legacy oversized payloads keep their immutable identity for reconciliation.
    /// They cannot be replayed within the wire budget and must not be acknowledged.
    pub fn oversized_usage_count(&self) -> Result<usize> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM usage_outbox
             WHERE acknowledged = 0 AND octet_length(batch) > ?1",
            [MAX_PENDING_USAGE_BYTES - ENVELOPE_RESERVE_BYTES],
            |row| row.get(0),
        )?;
        usize::try_from(count).context("oversized usage count out of range")
    }

    pub fn acknowledge_usage(&mut self, epoch: Uuid, seq: u64) -> Result<()> {
        self.connection.execute(
            "UPDATE usage_outbox SET acknowledged = 1 WHERE epoch = ?1 AND seq = ?2",
            params![epoch.to_string(), seq.to_string()],
        )?;
        Ok(())
    }

    pub fn pending_usage_count(&self) -> Result<usize> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM usage_outbox WHERE acknowledged = 0",
            [],
            |row| row.get(0),
        )?;
        usize::try_from(count).context("pending usage count out of range")
    }

    pub fn cleanup_acknowledged(&mut self) -> Result<()> {
        self.connection
            .execute("DELETE FROM usage_outbox WHERE acknowledged = 1", [])?;
        Ok(())
    }
}

fn persist_batch(
    transaction: &rusqlite::Transaction<'_>,
    epoch: Uuid,
    seq: &mut u64,
    period_start: i64,
    period_end: i64,
    records: Vec<UsageRecord>,
) -> Result<UsageBatch> {
    *seq = seq.checked_add(1).context("usage sequence exhausted")?;
    let batch = UsageBatch {
        epoch,
        seq: *seq,
        period_start,
        period_end,
        records,
    };
    transaction.execute(
        "INSERT INTO usage_outbox (epoch, seq, batch) VALUES (?1, ?2, ?3)",
        params![
            epoch.to_string(),
            seq.to_string(),
            serde_json::to_string(&batch)?
        ],
    )?;
    Ok(batch)
}

fn delta(current: u64, previous: u64, decreased: &mut bool) -> u64 {
    match current.checked_sub(previous) {
        Some(value) => value,
        None => {
            *decreased = true;
            current
        }
    }
}

fn clock_key(module: &str) -> String {
    format!("usage:module:{module}")
}

fn validate_module(module: &str) -> Result<()> {
    if module.is_empty() || module.len() > 256 || module.chars().any(char::is_control) {
        bail!("invalid usage module");
    }
    Ok(())
}

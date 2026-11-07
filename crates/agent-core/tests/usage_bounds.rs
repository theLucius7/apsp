#![forbid(unsafe_code)]

use apsp_adapter_sdk::Counter;
use apsp_agent_core::{
    state::State,
    usage::{MAX_PENDING_USAGE_BATCHES, MAX_PENDING_USAGE_BYTES, MAX_USAGE_BATCH_BYTES},
};
use apsp_protocol::{Envelope, UsageBatch, UsageRecord};
use rusqlite::{Connection, params};
use std::path::PathBuf;
use uuid::Uuid;

struct Database(PathBuf);

impl Database {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("apsp-usage-bounds-{}.db", Uuid::new_v4())))
    }

    fn open(&self) -> State {
        State::open(&self.0).unwrap()
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(format!("{}-wal", self.0.display()));
        let _ = std::fs::remove_file(format!("{}-shm", self.0.display()));
    }
}

fn batch(seq: u64, name: String) -> UsageBatch {
    UsageBatch {
        epoch: Uuid::from_u128(u128::from(seq)),
        seq,
        period_start: 10,
        period_end: 20,
        records: vec![UsageRecord {
            stat_name: name,
            uplink: 100,
            downlink: 200,
        }],
    }
}

fn insert(connection: &Connection, batch: &UsageBatch) {
    connection
        .execute(
            "INSERT INTO usage_outbox(epoch,seq,batch) VALUES (?1,?2,?3)",
            params![
                batch.epoch.to_string(),
                batch.seq.to_string(),
                serde_json::to_string(batch).unwrap()
            ],
        )
        .unwrap();
}

fn wire_bytes(batches: &[UsageBatch]) -> usize {
    batches
        .iter()
        .map(|batch| {
            serde_json::to_vec(&Envelope::new("usage.batch", batch).unwrap())
                .unwrap()
                .len()
        })
        .sum()
}

#[test]
fn thousands_of_batches_are_read_in_sql_limited_windows_and_replayed_exactly() {
    let database = Database::new();
    let mut state = database.open();
    let mut connection = Connection::open(&database.0).unwrap();
    let transaction = connection.transaction().unwrap();
    for seq in (1..=4_096).rev() {
        insert(&transaction, &batch(seq, format!("u{seq}_n1")));
    }
    transaction.commit().unwrap();
    let first = state.pending_usage().unwrap();
    assert_eq!(first.len(), MAX_PENDING_USAGE_BATCHES);
    assert_eq!(
        first.iter().map(|batch| batch.seq).collect::<Vec<_>>(),
        (1..=64).collect::<Vec<_>>()
    );
    assert!(wire_bytes(&first) <= MAX_PENDING_USAGE_BYTES);
    drop(state);
    state = database.open();
    assert_eq!(state.pending_usage().unwrap(), first);
    for batch in &first {
        state.acknowledge_usage(batch.epoch, batch.seq).unwrap();
        state.acknowledge_usage(batch.epoch, batch.seq).unwrap();
    }
    state.acknowledge_usage(Uuid::new_v4(), 65).unwrap();
    let next = state.pending_usage().unwrap();
    assert_eq!(next.first().unwrap().seq, 65);
    assert_eq!(next.last().unwrap().seq, 128);
    assert_eq!(state.pending_usage_count().unwrap(), 4_032);
    // The unread next row must never be deserialized as part of this window.
    connection
        .execute(
            "UPDATE usage_outbox SET batch='invalid-json' WHERE seq='129'",
            [],
        )
        .unwrap();
    assert_eq!(state.pending_usage().unwrap(), next);
}

#[test]
fn byte_budget_returns_an_ordered_prefix_and_does_not_starve_a_large_first_batch() {
    let database = Database::new();
    let mut state = database.open();
    let connection = Connection::open(&database.0).unwrap();
    let large = batch(1, "x".repeat(700 * 1024));
    insert(&connection, &large);
    for seq in 2..=20 {
        insert(&connection, &batch(seq, "x".repeat(90 * 1024)));
    }
    let first = state.pending_usage().unwrap();
    assert_eq!(first.first().unwrap(), &large);
    assert_eq!(first.len(), 4);
    assert!(wire_bytes(&first) <= MAX_PENDING_USAGE_BYTES);
    for batch in first {
        state.acknowledge_usage(batch.epoch, batch.seq).unwrap();
    }
    let second = state.pending_usage().unwrap();
    assert_eq!(second.first().unwrap().seq, 5);
    assert!(second.len() < MAX_PENDING_USAGE_BATCHES);
    assert!(wire_bytes(&second) <= MAX_PENDING_USAGE_BYTES);

    // Keep the SQL/index literal aligned with the byte budget, including the
    // exact largest sendable payload and the first byte that cannot fit.
    let boundary_database = Database::new();
    let boundary_state = boundary_database.open();
    let connection = Connection::open(&boundary_database.0).unwrap();
    let empty = batch(1, String::new());
    let overhead = serde_json::to_vec(&empty).unwrap().len();
    let largest = batch(1, "x".repeat(MAX_PENDING_USAGE_BYTES - 128 - overhead));
    assert_eq!(
        serde_json::to_vec(&largest).unwrap().len(),
        MAX_PENDING_USAGE_BYTES - 128
    );
    insert(&connection, &largest);
    let first_blocked = batch(2, format!("{}x", largest.records[0].stat_name));
    insert(&connection, &first_blocked);
    let selected = boundary_state.pending_usage().unwrap();
    assert_eq!(selected, vec![largest]);
    assert!(wire_bytes(&selected) <= MAX_PENDING_USAGE_BYTES);
    assert_eq!(boundary_state.oversized_usage_count().unwrap(), 1);
}

#[test]
fn legacy_oversized_payload_is_never_loaded_or_acknowledged_and_later_batches_progress() {
    let database = Database::new();
    let mut state = database.open();
    let connection = Connection::open(&database.0).unwrap();
    // Invalid oversized JSON proves selection examines byte metadata before decoding.
    let oversized = "x".repeat(2 * MAX_PENDING_USAGE_BYTES);
    connection
        .execute(
            "INSERT INTO usage_outbox(epoch,seq,batch) VALUES (?1,'1',?2)",
            params![Uuid::nil().to_string(), oversized],
        )
        .unwrap();
    let later = batch(2, "u1_n1".into());
    insert(&connection, &later);
    assert_eq!(state.oversized_usage_count().unwrap(), 1);
    assert_eq!(state.pending_usage().unwrap(), vec![later.clone()]);
    state.acknowledge_usage(later.epoch, later.seq).unwrap();
    assert!(state.pending_usage().unwrap().is_empty());
    assert_eq!(state.pending_usage_count().unwrap(), 1);
    state.cleanup_acknowledged().unwrap();
    drop(state);
    let reopened = database.open();
    assert_eq!(reopened.oversized_usage_count().unwrap(), 1);
    let saved: String = connection
        .query_row("SELECT batch FROM usage_outbox WHERE seq='1'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(saved, oversized);
}

#[test]
fn large_samples_split_atomically_without_losing_records_or_advancing_baselines_twice() {
    let database = Database::new();
    let mut state = database.open();
    let counters: Vec<_> = (0..3_000)
        .map(|index| Counter {
            stat_name: format!("counter_{index:04}{}", "\\".repeat(490)),
            uplink: u64::MAX,
            downlink: u64::MAX - 1,
        })
        .collect();
    let first = state
        .sample_usage("runtime", &counters, 10)
        .unwrap()
        .unwrap();
    let total_batches = state.pending_usage_count().unwrap();
    assert!(total_batches > 1);
    assert!(total_batches < MAX_PENDING_USAGE_BATCHES);
    let mut records = Vec::new();
    let mut next_seq = 1;
    let mut last_batch = None;
    while state.pending_usage_count().unwrap() != 0 {
        let window = state.pending_usage().unwrap();
        assert!(!window.is_empty());
        assert!(wire_bytes(&window) <= MAX_PENDING_USAGE_BYTES);
        for batch in window {
            assert_eq!(batch.seq, next_seq);
            next_seq += 1;
            assert_eq!(batch.epoch, first.epoch);
            assert_eq!((batch.period_start, batch.period_end), (10, 10));
            assert!(wire_bytes(std::slice::from_ref(&batch)) <= MAX_USAGE_BATCH_BYTES);
            records.extend(batch.records.clone());
            state.acknowledge_usage(batch.epoch, batch.seq).unwrap();
            last_batch = Some(batch);
        }
        drop(state);
        state = database.open();
    }
    assert_eq!(records.len(), counters.len());
    for (record, counter) in records.iter().zip(counters.iter()) {
        assert_eq!(record.stat_name, counter.stat_name);
        assert_eq!(
            (record.uplink, record.downlink),
            (counter.uplink, counter.downlink)
        );
    }
    assert!(
        state
            .sample_usage("runtime", &counters, 20)
            .unwrap()
            .is_none()
    );
    let increment = state
        .sample_usage(
            "runtime",
            &[Counter {
                downlink: u64::MAX,
                ..counters[0].clone()
            }],
            30,
        )
        .unwrap()
        .unwrap();
    assert_eq!(increment.seq, last_batch.unwrap().seq + 1);
    assert_eq!(
        (increment.records[0].uplink, increment.records[0].downlink),
        (0, 1)
    );
    assert_eq!(increment.period_start, 20);
}

#[test]
fn sequence_exhaustion_in_a_later_chunk_rolls_back_the_entire_sample() {
    let database = Database::new();
    let mut state = database.open();
    state.set_json("usage:last_seq", &(u64::MAX - 1)).unwrap();
    let counters: Vec<_> = (0..300)
        .map(|index| Counter {
            stat_name: format!("{index:04}{}", "\\".repeat(490)),
            uplink: 5,
            downlink: 10,
        })
        .collect();
    assert!(state.sample_usage("runtime", &counters, 10).is_err());
    assert_eq!(state.pending_usage_count().unwrap(), 0);
    assert_eq!(
        state.get_json::<u64>("usage:last_seq").unwrap(),
        Some(u64::MAX - 1)
    );
    state.set_json("usage:last_seq", &0_u64).unwrap();
    let first = state
        .sample_usage("runtime", &counters, 20)
        .unwrap()
        .unwrap();
    assert_eq!(
        (first.seq, first.period_start, first.period_end),
        (1, 20, 20)
    );
    assert!(
        first
            .records
            .iter()
            .all(|record| (record.uplink, record.downlink) == (5, 10))
    );
}

#[test]
fn failed_outbox_write_rolls_back_chunks_and_cumulative_baselines() {
    let database = Database::new();
    let mut state = database.open();
    let connection = Connection::open(&database.0).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_second_chunk BEFORE INSERT ON usage_outbox WHEN NEW.seq='2'
         BEGIN SELECT RAISE(ABORT, 'injected database or disk full'); END;",
        )
        .unwrap();
    let counters: Vec<_> = (0..300)
        .map(|index| Counter {
            stat_name: format!("{index:04}{}", "\\".repeat(490)),
            uplink: 5,
            downlink: 10,
        })
        .collect();
    assert!(state.sample_usage("runtime", &counters, 10).is_err());
    assert_eq!(state.pending_usage_count().unwrap(), 0);
    assert_eq!(state.get_json::<u64>("usage:last_seq").unwrap(), None);
    connection
        .execute_batch("DROP TRIGGER reject_second_chunk")
        .unwrap();
    let first = state
        .sample_usage("runtime", &counters, 20)
        .unwrap()
        .unwrap();
    assert_eq!(
        (first.seq, first.period_start, first.period_end),
        (1, 20, 20)
    );
    assert!(
        first
            .records
            .iter()
            .all(|record| (record.uplink, record.downlink) == (5, 10))
    );
}

#[test]
fn upgrading_an_existing_ledger_adds_indexes_without_rewriting_batch_identity() {
    let database = Database::new();
    let mut connection = Connection::open(&database.0).unwrap();
    connection
        .execute_batch(include_str!("../src/state/migrations/0001.sql"))
        .unwrap();
    connection.pragma_update(None, "user_version", 1).unwrap();
    let saved = batch(1, "u1_n1".into());
    insert(&connection, &saved);
    let state = database.open();
    assert_eq!(state.pending_usage().unwrap(), vec![saved.clone()]);
    let migration: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(migration, 1);
    let plan: String = connection.query_row(
        "EXPLAIN QUERY PLAN SELECT seq FROM usage_outbox INDEXED BY usage_outbox_pending_order_idx WHERE acknowledged=0 AND octet_length(batch)<=1048447 ORDER BY length(seq),seq LIMIT 64",
        [], |row| row.get(3),
    ).unwrap();
    assert!(plan.contains("usage_outbox_pending_order_idx"));
    // An old Agent's migration registry must still accept the ledger on rollback.
    rusqlite_migration::Migrations::new(vec![rusqlite_migration::M::up(include_str!(
        "../src/state/migrations/0001.sql"
    ))])
    .to_latest(&mut connection)
    .unwrap();
    assert_eq!(database.open().pending_usage().unwrap(), vec![saved]);
}

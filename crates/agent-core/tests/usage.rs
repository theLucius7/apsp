#![forbid(unsafe_code)]

use apsp_adapter_sdk::Counter;
use apsp_agent_core::state::{IntentRecord, State};
use std::path::PathBuf;
use uuid::Uuid;

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(
            std::env::temp_dir()
                .join(format!("apsp-ledger-{}", Uuid::new_v4()))
                .join("state.db"),
        )
    }
    fn open(&self) -> State {
        State::open(&self.0).unwrap()
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        if let Some(parent) = self.0.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}
fn counter(name: &str, uplink: u64, downlink: u64) -> Counter {
    Counter {
        stat_name: name.into(),
        uplink,
        downlink,
    }
}

#[test]
fn differences_are_durable_and_unchanged_samples_advance_time_without_sequence() {
    let database = Database::new();
    let mut state = database.open();
    let first = state
        .sample_usage("runtime", &[counter("u1_n1", 100, 200)], 10)
        .unwrap()
        .unwrap();
    assert_eq!(
        (first.seq, first.period_start, first.period_end),
        (1, 10, 10)
    );
    assert_eq!(
        (first.records[0].uplink, first.records[0].downlink),
        (100, 200)
    );
    let second = state
        .sample_usage("runtime", &[counter("u1_n1", 160, 260)], 40)
        .unwrap()
        .unwrap();
    assert_eq!(
        (second.seq, second.period_start, second.period_end),
        (2, 10, 40)
    );
    assert_eq!(
        (second.records[0].uplink, second.records[0].downlink),
        (60, 60)
    );
    assert!(
        state
            .sample_usage("runtime", &[counter("u1_n1", 160, 260)], 70)
            .unwrap()
            .is_none()
    );
    let third = state
        .sample_usage("runtime", &[counter("u1_n1", 170, 270)], 100)
        .unwrap()
        .unwrap();
    assert_eq!((third.seq, third.period_start), (3, 70));
    assert_eq!(third.epoch, first.epoch);
}

#[test]
fn explicit_epoch_starts_from_zero_and_preserves_unsent_batches() {
    let database = Database::new();
    let mut state = database.open();
    let first = state
        .sample_usage("runtime", &[counter("u1_n1", 100, 200)], 10)
        .unwrap()
        .unwrap();
    state.begin_usage_epoch("runtime", 20).unwrap();
    let second = state
        .sample_usage("runtime", &[counter("u1_n1", 5, 8)], 25)
        .unwrap()
        .unwrap();
    assert_ne!(second.epoch, first.epoch);
    assert_eq!((second.seq, second.period_start), (2, 20));
    assert_eq!(
        (second.records[0].uplink, second.records[0].downlink),
        (5, 8)
    );
    assert_eq!(state.pending_usage().unwrap(), vec![first, second]);
}

#[test]
fn unexpected_reset_counts_other_series_and_directions_only_once() {
    let database = Database::new();
    let mut state = database.open();
    let first = state
        .sample_usage(
            "runtime",
            &[counter("a", 100, 200), counter("b", 300, 400)],
            10,
        )
        .unwrap()
        .unwrap();
    let second = state
        .sample_usage(
            "runtime",
            &[counter("b", 330, 450), counter("a", 4, 240)],
            20,
        )
        .unwrap()
        .unwrap();
    assert_ne!(second.epoch, first.epoch);
    assert_eq!(
        (
            second.records[0].stat_name.as_str(),
            second.records[0].uplink,
            second.records[0].downlink
        ),
        ("a", 4, 40)
    );
    assert_eq!(
        (second.records[1].uplink, second.records[1].downlink),
        (30, 50)
    );
    let third = state
        .sample_usage(
            "runtime",
            &[counter("a", 9, 250), counter("b", 350, 470)],
            30,
        )
        .unwrap()
        .unwrap();
    assert_eq!(third.epoch, second.epoch);
    assert_eq!(
        (third.records[0].uplink, third.records[0].downlink),
        (5, 10)
    );
    assert_eq!(
        (third.records[1].uplink, third.records[1].downlink),
        (20, 20)
    );
}

#[test]
fn temporarily_missing_series_keep_their_baseline_across_a_partial_reset() {
    let database = Database::new();
    let mut state = database.open();
    state
        .sample_usage(
            "runtime",
            &[counter("a", 100, 100), counter("b", 200, 200)],
            10,
        )
        .unwrap();
    state
        .sample_usage("runtime", &[counter("a", 2, 2)], 20)
        .unwrap();
    let batch = state
        .sample_usage("runtime", &[counter("b", 220, 240)], 30)
        .unwrap()
        .unwrap();
    assert_eq!(
        (batch.records[0].uplink, batch.records[0].downlink),
        (20, 40)
    );
}

#[test]
fn duplicate_and_unknown_acknowledgments_are_idempotent() {
    let database = Database::new();
    let mut state = database.open();
    let batch = state
        .sample_usage("runtime", &[counter("a", 100, 200)], 10)
        .unwrap()
        .unwrap();
    state.acknowledge_usage(Uuid::new_v4(), batch.seq).unwrap();
    assert_eq!(state.pending_usage_count().unwrap(), 1);
    state.acknowledge_usage(batch.epoch, batch.seq).unwrap();
    state.acknowledge_usage(batch.epoch, batch.seq).unwrap();
    assert_eq!(state.pending_usage_count().unwrap(), 0);
    state.cleanup_acknowledged().unwrap();
    state.acknowledge_usage(batch.epoch, batch.seq).unwrap();
    let next = state
        .sample_usage("runtime", &[counter("a", 120, 220)], 20)
        .unwrap()
        .unwrap();
    assert_eq!(next.seq, 2);
    assert_eq!((next.records[0].uplink, next.records[0].downlink), (20, 20));
}

#[test]
fn reopen_resends_exact_batches_and_preserves_baselines() {
    let database = Database::new();
    let first = {
        let mut state = database.open();
        state
            .sample_usage("runtime", &[counter("a", 100, 200)], 10)
            .unwrap()
            .unwrap()
    };
    let mut reopened = database.open();
    assert_eq!(reopened.pending_usage().unwrap(), vec![first.clone()]);
    let second = reopened
        .sample_usage("runtime", &[counter("a", 130, 230)], 20)
        .unwrap()
        .unwrap();
    assert_eq!(second.epoch, first.epoch);
    assert_eq!(second.seq, 2);
    assert_eq!(
        (second.records[0].uplink, second.records[0].downlink),
        (30, 30)
    );
}

#[test]
fn duplicate_sample_and_sequence_overflow_roll_back_every_change() {
    let database = Database::new();
    let mut state = database.open();
    state
        .sample_usage("runtime", &[counter("a", 100, 200)], 10)
        .unwrap();
    assert!(
        state
            .sample_usage(
                "runtime",
                &[counter("a", 110, 210), counter("a", 120, 220)],
                20
            )
            .is_err()
    );
    state.set_json("usage:last_seq", &u64::MAX).unwrap();
    assert!(
        state
            .sample_usage("runtime", &[counter("a", 5, 250)], 20)
            .is_err()
    );
    assert_eq!(state.pending_usage_count().unwrap(), 1);
    state.set_json("usage:last_seq", &1_u64).unwrap();
    let batch = state
        .sample_usage("runtime", &[counter("a", 130, 260)], 30)
        .unwrap()
        .unwrap();
    assert_eq!((batch.seq, batch.period_start), (2, 10));
    assert_eq!(
        (batch.records[0].uplink, batch.records[0].downlink),
        (30, 60)
    );
    assert_eq!(state.pending_usage().unwrap()[0].epoch, batch.epoch);
}

#[test]
fn full_width_counters_do_not_overflow_sqlite_integers() {
    let database = Database::new();
    let mut state = database.open();
    let batch = state
        .sample_usage("runtime", &[counter("a", u64::MAX, u64::MAX - 1)], 10)
        .unwrap()
        .unwrap();
    assert_eq!(batch.records[0].uplink, u64::MAX);
    drop(state);
    let mut state = database.open();
    assert!(
        state
            .sample_usage("runtime", &[counter("a", u64::MAX, u64::MAX - 1)], 20)
            .unwrap()
            .is_none()
    );
    let batch = state
        .sample_usage("runtime", &[counter("a", u64::MAX, u64::MAX)], 30)
        .unwrap()
        .unwrap();
    assert_eq!((batch.records[0].uplink, batch.records[0].downlink), (0, 1));
}

#[test]
fn global_sequences_are_ordered_across_modules_and_epochs() {
    let database = Database::new();
    let mut state = database.open();
    for index in 1..=12 {
        state.begin_usage_epoch("runtime", index).unwrap();
        state
            .sample_usage("runtime", &[counter("a", 1, 0)], index)
            .unwrap();
    }
    state
        .sample_usage("other", &[counter("a", 1, 0)], 20)
        .unwrap();
    let sequences: Vec<_> = state
        .pending_usage()
        .unwrap()
        .iter()
        .map(|batch| batch.seq)
        .collect();
    assert_eq!(sequences, (1..=13).collect::<Vec<_>>());
}

#[test]
fn backwards_time_does_not_change_ledger_or_epoch() {
    let database = Database::new();
    let mut state = database.open();
    let first = state
        .sample_usage("runtime", &[counter("a", 100, 100)], 10)
        .unwrap()
        .unwrap();
    assert!(
        state
            .sample_usage("runtime", &[counter("a", 110, 110)], 9)
            .is_err()
    );
    assert!(state.begin_usage_epoch("runtime", 9).is_err());
    let second = state
        .sample_usage("runtime", &[counter("a", 120, 120)], 20)
        .unwrap()
        .unwrap();
    assert_eq!(second.epoch, first.epoch);
    assert_eq!(second.records[0].uplink, 20);
}

#[test]
fn state_migrations_kv_and_intents_survive_reopening() {
    let database = Database::new();
    let intent = IntentRecord {
        op_id: Uuid::new_v4(),
        module: "runtime".into(),
        payload: serde_json::json!({"target": 7}),
    };
    {
        let mut state = database.open();
        state.set_json("health:runtime", &true).unwrap();
        state.begin_intent(&intent).unwrap();
        state.begin_intent(&intent).unwrap();
    }
    let mut state = database.open();
    assert_eq!(
        state.get_json::<bool>("health:runtime").unwrap(),
        Some(true)
    );
    assert_eq!(state.pending_intents().unwrap(), vec![intent.clone()]);
    state.finish_intent(intent.op_id).unwrap();
    state.finish_intent(intent.op_id).unwrap();
    assert!(state.pending_intents().unwrap().is_empty());
    drop(state);
    let connection = rusqlite::Connection::open(&database.0).unwrap();
    let migration: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let journal: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(migration, 1);
    assert_eq!(journal, "wal");
}

#[test]
fn completing_an_intent_and_updating_kv_are_atomic() {
    let database = Database::new();
    let mut state = database.open();
    let intent = IntentRecord {
        op_id: Uuid::new_v4(),
        module: "runtime".into(),
        payload: serde_json::json!({"target": 7}),
    };
    state.begin_intent(&intent).unwrap();
    state.set_json("health:runtime", &false).unwrap();
    let connection = rusqlite::Connection::open(&database.0).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_completion BEFORE UPDATE ON intents BEGIN SELECT RAISE(ABORT, 'injected transaction failure'); END;").unwrap();
    assert!(
        state
            .complete_intent(
                intent.op_id,
                &[("health:runtime".into(), serde_json::json!(true))]
            )
            .is_err()
    );
    assert_eq!(
        state.get_json::<bool>("health:runtime").unwrap(),
        Some(false)
    );
    assert_eq!(state.pending_intents().unwrap(), vec![intent.clone()]);
    connection
        .execute_batch("DROP TRIGGER reject_completion")
        .unwrap();
    state
        .complete_intent(
            intent.op_id,
            &[("health:runtime".into(), serde_json::json!(true))],
        )
        .unwrap();
    assert_eq!(
        state.get_json::<bool>("health:runtime").unwrap(),
        Some(true)
    );
    assert!(state.pending_intents().unwrap().is_empty());
    state.remove_json("health:runtime").unwrap();
    assert_eq!(state.get_json::<bool>("health:runtime").unwrap(), None);
}

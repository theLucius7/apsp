#![forbid(unsafe_code)]

use apsp_agent_core::state::State;
use rusqlite::Connection;
use std::path::PathBuf;

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("apsp-baseline-{}.db", uuid::Uuid::new_v4())))
    }
    fn rejected(&self) {
        let error = State::open(&self.0)
            .err()
            .expect("incompatible database must fail");
        assert!(format!("{error:#}").contains("手动重建"));
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

#[test]
fn old_and_unknown_databases_are_untouched() {
    for version in [0, 1, 99] {
        let database = Database::new();
        let connection = Connection::open(&database.0).unwrap();
        connection.execute_batch(&format!("CREATE TABLE saved(value TEXT); INSERT INTO saved VALUES('TEST_ONLY retained'); PRAGMA user_version={version}")).unwrap();
        drop(connection);
        let bytes = std::fs::read(&database.0).unwrap();
        database.rejected();
        assert_eq!(bytes, std::fs::read(&database.0).unwrap());
    }
}

#[test]
fn current_records_survive_reopening_and_partial_schema_is_rejected() {
    let database = Database::new();
    let mut state = State::open(&database.0).unwrap();
    state.set_json("TEST_ONLY retained", &42).unwrap();
    drop(state);
    assert_eq!(
        State::open(&database.0)
            .unwrap()
            .get_json::<i64>("TEST_ONLY retained")
            .unwrap(),
        Some(42)
    );
    let connection = Connection::open(&database.0).unwrap();
    connection
        .execute_batch("DROP INDEX usage_outbox_pending_order_idx")
        .unwrap();
    database.rejected();
    assert_eq!(
        connection
            .query_row(
                "SELECT value FROM kv WHERE key='TEST_ONLY retained'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "42"
    );
}

#[test]
fn mismatched_source_identity_is_not_overwritten() {
    let database = Database::new();
    drop(State::open(&database.0).unwrap());
    let connection = Connection::open(&database.0).unwrap();
    connection
        .execute(
            "UPDATE schema_identity SET source_hash='TEST_ONLY old baseline'",
            [],
        )
        .unwrap();
    database.rejected();
    assert_eq!(
        connection
            .query_row("SELECT source_hash FROM schema_identity", [], |row| row
                .get::<_, String>(
                0
            ))
            .unwrap(),
        "TEST_ONLY old baseline"
    );
}

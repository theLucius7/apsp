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

/// The ledger exactly as Agent 0.3.7 left it on the installed servers.
const FLEET: &str = include_str!("fixtures/state-0.3.7.sql");

fn schema(path: &PathBuf) -> Vec<String> {
    let connection = Connection::open(path).unwrap();
    let mut query = connection
        .prepare("SELECT type || ':' || name || ':' || sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name")
        .unwrap();
    query
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn the_installed_ledger_is_recorded_as_current_without_changing_tables() {
    let database = Database::new();
    let connection = Connection::open(&database.0).unwrap();
    connection
        .execute_batch("PRAGMA journal_mode = WAL;")
        .unwrap();
    connection.execute_batch(FLEET).unwrap();
    connection
        .execute_batch(
            "PRAGMA user_version = 1;
             INSERT INTO kv VALUES('TEST_ONLY retained','42');
             INSERT INTO usage_outbox(epoch,seq,batch) VALUES('TEST_ONLY epoch','1','{}');
             INSERT INTO command_journal(id,spec) VALUES('TEST_ONLY command','{}');
             INSERT INTO command_lifecycle(id,claim_id) VALUES('TEST_ONLY command','TEST_ONLY claim');",
        )
        .unwrap();
    drop(connection);

    let state = State::open(&database.0).unwrap();
    assert_eq!(
        state.get_json::<i64>("TEST_ONLY retained").unwrap(),
        Some(42)
    );
    drop(state);
    // Afterwards the file opens like any current ledger and equals a fresh one.
    drop(State::open(&database.0).unwrap());
    let fresh = Database::new();
    drop(State::open(&fresh.0).unwrap());
    assert_eq!(schema(&database.0), schema(&fresh.0));
    let identity = |path: &PathBuf| {
        let connection = Connection::open(path).unwrap();
        let hashes: (String, String) = connection
            .query_row(
                "SELECT source_hash,catalog_hash FROM schema_identity",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let application: i64 = connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .unwrap();
        (hashes, version, application)
    };
    assert_eq!(identity(&database.0), identity(&fresh.0));
    let connection = Connection::open(&database.0).unwrap();
    for (table, rows) in [
        ("usage_outbox", 1),
        ("command_journal", 1),
        ("command_lifecycle", 1),
    ] {
        let count: i64 = connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, rows, "{table}");
    }
}

#[test]
fn a_version_one_ledger_with_another_structure_is_refused_untouched() {
    for change in [
        "DROP INDEX usage_outbox_pending_order_idx",
        "CREATE TABLE saved(value TEXT)",
    ] {
        let database = Database::new();
        let connection = Connection::open(&database.0).unwrap();
        connection.execute_batch(FLEET).unwrap();
        connection
            .execute_batch(&format!("{change}; PRAGMA user_version = 1;"))
            .unwrap();
        drop(connection);
        let bytes = std::fs::read(&database.0).unwrap();
        database.rejected();
        assert_eq!(bytes, std::fs::read(&database.0).unwrap(), "{change}");
    }
}

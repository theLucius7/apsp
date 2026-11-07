use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, TransactionBehavior};
use sha2::{Digest, Sha256};

const VERSION: i64 = 2;
const APPLICATION: i64 = 0x41505350;
const SQL: &str = include_str!("schema.sql");
const REBUILD: &str = "Agent 数据库不是当前开发基线或结构不完整，已拒绝启动。请先停止服务并备份，再手动重建状态目录、重新登记设备；程序不会自动删除数据。";

fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn catalog_rows(connection: &Connection) -> Result<Vec<String>> {
    let mut query = connection.prepare("SELECT type || ':' || name || ':' || sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' AND sql IS NOT NULL ORDER BY type, name")?;
    let rows = query
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn catalog(connection: &Connection) -> Result<String> {
    Ok(digest(&catalog_rows(connection)?.join("\n")))
}

/// Agents up to 0.3.7 created exactly these tables and indexes from separate files and
/// marked the file version 1 (ADR 0122 supplement). When an existing version 1 file
/// already has the current structure apart from the identity table, returns the
/// identity table's definition so the file can be recorded as the current baseline
/// without changing a table; any other difference is refused.
fn identity_missing_only(connection: &Connection) -> Result<Option<String>> {
    let current = Connection::open_in_memory()?;
    current.execute_batch(SQL)?;
    let identity: String = current.query_row(
        "SELECT sql FROM sqlite_schema WHERE type='table' AND name='schema_identity'",
        [],
        |row| row.get(0),
    )?;
    let mut expected = catalog_rows(&current)?;
    expected.retain(|row| !row.starts_with("table:schema_identity:"));
    Ok((catalog_rows(connection)? == expected).then_some(identity))
}

fn record_identity(connection: &Connection) -> Result<()> {
    connection.execute(
        "INSERT INTO schema_identity VALUES(1,?1,?2)",
        [digest(SQL), catalog(connection)?],
    )?;
    connection.pragma_update(None, "user_version", VERSION)?;
    connection.pragma_update(None, "application_id", APPLICATION)?;
    Ok(())
}

pub(super) fn initialize(connection: &mut Connection) -> Result<()> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let application: i64 = tx.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let objects: i64 = tx.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    if objects == 0 && version == 0 && application == 0 {
        tx.execute_batch(SQL)
            .context("initialize current state baseline")?;
        record_identity(&tx)?;
    } else if version == 1 && application == 0 {
        // The installed fleet's ledger: same structure, no identity record yet.
        let identity = identity_missing_only(&tx)?.context(REBUILD)?;
        tx.execute_batch(&identity)
            .context("record current state baseline")?;
        record_identity(&tx)?;
    } else {
        ensure!(version == VERSION && application == APPLICATION, REBUILD);
        let (source, structure): (String, String) = tx
            .query_row(
                "SELECT source_hash,catalog_hash FROM schema_identity WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .context(REBUILD)?;
        ensure!(source == digest(SQL) && structure == catalog(&tx)?, REBUILD);
    }
    tx.commit()?;
    Ok(())
}

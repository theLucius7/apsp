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

fn catalog(connection: &Connection) -> Result<String> {
    let mut query = connection.prepare("SELECT type || ':' || name || ':' || sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' AND sql IS NOT NULL ORDER BY type, name")?;
    let rows = query
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(digest(&rows.join("\n")))
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
        tx.execute(
            "INSERT INTO schema_identity VALUES(1,?1,?2)",
            [digest(SQL), catalog(&tx)?],
        )?;
        tx.pragma_update(None, "user_version", VERSION)?;
        tx.pragma_update(None, "application_id", APPLICATION)?;
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

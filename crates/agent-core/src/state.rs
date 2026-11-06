use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::{M, Migrations};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use uuid::Uuid;

pub mod runtime_control;

pub type SharedState = Arc<Mutex<State>>;

/// Locks the shared Agent state; a panic while it was held becomes an error.
pub(crate) trait Locked {
    fn locked(&self) -> anyhow::Result<std::sync::MutexGuard<'_, State>>;
}

impl Locked for Mutex<State> {
    fn locked(&self) -> anyhow::Result<std::sync::MutexGuard<'_, State>> {
        self.lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))
    }
}

mod storage;
pub(crate) use storage::StorageRetry;

pub struct State {
    pub(crate) connection: Connection,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IntentRecord {
    pub op_id: Uuid,
    pub module: String,
    pub payload: serde_json::Value,
}

impl State {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).context("create state directory")?;
        }
        let mut connection = Connection::open(path).context("open state database")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if path != Path::new(":memory:") {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            }
        }
        connection.busy_timeout(Duration::from_secs(10))?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;",
        )?;
        Migrations::new(vec![M::up(include_str!("state/migrations/0001.sql"))])
            .to_latest(&mut connection)?;
        // Auxiliary tables remain additive so an older Agent can reopen the ledger after rollback.
        connection.execute_batch(include_str!("state/migrations/0002.sql"))?;
        connection.execute_batch(include_str!("state/migrations/0003.sql"))?;
        connection.execute_batch(include_str!("state/migrations/0004_bounded_usage.sql"))?;
        connection.execute_batch(include_str!("state/migrations/0005_runtime_control.sql"))?;
        connection.execute_batch(include_str!("state/migrations/0005_command_lifecycle.sql"))?;
        Ok(Self { connection })
    }

    pub fn get_json<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        read_json(&self.connection, key)
    }

    pub fn set_json<T: Serialize>(&mut self, key: &str, value: &T) -> Result<()> {
        write_json(&self.connection, key, value)
    }

    pub fn set_json_batch(&mut self, updates: &[(String, serde_json::Value)]) -> Result<()> {
        let transaction = self.connection.transaction()?;
        for (key, value) in updates {
            write_json(&transaction, key, value)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn remove_json(&mut self, key: &str) -> Result<()> {
        self.connection
            .execute("DELETE FROM kv WHERE key = ?1", [key])?;
        Ok(())
    }

    pub fn begin_intent(&mut self, intent: &IntentRecord) -> Result<()> {
        let payload = serde_json::to_string(&intent.payload)?;
        let existing: Option<(String, String, bool)> = self
            .connection
            .query_row(
                "SELECT module, payload, completed FROM intents WHERE op_id = ?1",
                [intent.op_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if let Some((module, saved, completed)) = existing {
            if module != intent.module
                || serde_json::from_str::<serde_json::Value>(&saved)? != intent.payload
                || completed
            {
                bail!("intent identifier already exists with different or completed work");
            }
            return Ok(());
        }
        self.connection.execute(
            "INSERT INTO intents (op_id, module, payload) VALUES (?1, ?2, ?3)",
            params![intent.op_id.to_string(), intent.module, payload],
        )?;
        Ok(())
    }

    pub fn finish_intent(&mut self, op_id: Uuid) -> Result<()> {
        self.complete_intent(op_id, &[])
    }

    pub fn complete_intent(
        &mut self,
        op_id: Uuid,
        updates: &[(String, serde_json::Value)],
    ) -> Result<()> {
        let transaction = self.connection.transaction()?;
        for (key, value) in updates {
            write_json(&transaction, key, value)?;
        }
        transaction.execute(
            "UPDATE intents SET completed = 1 WHERE op_id = ?1",
            [op_id.to_string()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn clear_retired_configuration(&mut self) -> Result<()> {
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM intents", [])?;
        transaction.execute("DELETE FROM command_journal", [])?;
        transaction.execute("DELETE FROM probe_outbox", [])?;
        transaction.execute("DELETE FROM telemetry_outbox", [])?;
        transaction.execute("DELETE FROM runtime_control", [])?;
        transaction.execute("DELETE FROM runtime_revision_floors", [])?;
        transaction.execute(
            "DELETE FROM kv WHERE key NOT LIKE 'usage:%' AND key <> 'retirement'",
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn pending_intents(&self) -> Result<Vec<IntentRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT op_id, module, payload FROM intents WHERE completed = 0 ORDER BY rowid",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (op_id, module, payload) = row?;
            Ok(IntentRecord {
                op_id: Uuid::parse_str(&op_id)?,
                module,
                payload: serde_json::from_str(&payload)?,
            })
        })
        .collect()
    }
}

pub(crate) fn read_json<T: DeserializeOwned>(
    connection: &Connection,
    key: &str,
) -> Result<Option<T>> {
    let text: Option<String> = connection
        .query_row("SELECT value FROM kv WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?;
    text.map(|text| serde_json::from_str(&text).context("decode saved state value"))
        .transpose()
}

pub(crate) fn write_json<T: Serialize>(
    connection: &Connection,
    key: &str,
    value: &T,
) -> Result<()> {
    connection.execute("INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value", params![key, serde_json::to_string(value)?])?;
    Ok(())
}

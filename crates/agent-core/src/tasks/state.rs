use crate::State;
use anyhow::{Result, ensure};
use apsp_adapter_sdk::CommandProcessIdentity;
use apsp_protocol::{
    CommandResult, CommandStarted, CommandStatus, ProbeResult, RemoteCommand, now_timestamp,
    telemetry::now_millis,
};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

pub(super) struct CommandRecord {
    pub command: RemoteCommand,
    pub claim_id: Uuid,
    pub process: Option<CommandProcessIdentity>,
    pub cancel_requested: bool,
    pub recovering: bool,
}

impl State {
    pub fn begin_command(&mut self, command: &RemoteCommand) -> Result<bool> {
        ensure!(command.valid(), "invalid remote command");
        let previous: Option<String> = self
            .connection
            .query_row(
                "SELECT spec FROM command_journal WHERE id=?1",
                [command.id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(previous) = previous {
            ensure!(
                serde_json::from_str::<RemoteCommand>(&previous)? == *command,
                "command identifier changed content"
            );
            return Ok(false);
        }
        self.connection.execute("DELETE FROM command_journal WHERE acknowledged=1 AND CAST(json_extract(spec,'$.expires_at') AS INTEGER)<?1 AND id NOT IN (SELECT id FROM command_lifecycle WHERE started IS NOT NULL AND started_acknowledged=0)", [now_timestamp()-7*86400])?;
        let count: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM command_journal", [], |r| r.get(0))?;
        ensure!(count < 10_000, "command journal reached retention limit");
        self.connection.execute(
            "INSERT INTO command_journal(id,spec) VALUES(?1,?2)",
            params![command.id.to_string(), serde_json::to_string(command)?],
        )?;
        Ok(true)
    }

    pub fn finish_command(&mut self, result: &CommandResult) -> Result<()> {
        let started: Option<Option<String>> = self
            .connection
            .query_row(
                "SELECT started FROM command_lifecycle WHERE id=?1",
                [result.id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let mut result = result.clone();
        if let Some(started) = started.flatten() {
            result.finished_at = result
                .finished_at
                .max(serde_json::from_str::<CommandStarted>(&started)?.started_at);
        }
        let updated = self.connection.execute(
            "UPDATE command_journal SET result=?2 WHERE id=?1 AND result IS NULL",
            params![result.id.to_string(), serde_json::to_string(&result)?],
        )?;
        ensure!(updated == 1, "command was not running or already finished");
        Ok(())
    }

    pub fn recover_commands(&mut self) -> Result<()> {
        self.connection.execute("UPDATE command_lifecycle SET recovering=1 WHERE id IN (SELECT id FROM command_journal WHERE result IS NULL)", [])?;
        let ids = {
            let mut query = self
                .connection
                .prepare("SELECT id FROM command_journal WHERE result IS NULL AND id NOT IN (SELECT id FROM command_lifecycle)")?;
            query
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for id in ids {
            self.finish_command(&CommandResult {
                id: Uuid::parse_str(&id)?,
                status: CommandStatus::Interrupted,
                finished_at: now_timestamp()
                    + self.get_json::<i64>("clock_offset_ms")?.unwrap_or(0) / 1000,
                stdout: String::new(),
                stderr: "Agent restarted; command was not executed again".into(),
                timed_out: false,
                truncated: false,
            })?;
        }
        Ok(())
    }

    pub fn command_results(&self) -> Result<Vec<CommandResult>> {
        let mut query = self.connection.prepare("SELECT result FROM command_journal WHERE result IS NOT NULL AND acknowledged=0 ORDER BY rowid LIMIT 64")?;
        let rows = query
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .map(|v| serde_json::from_str(&v))
            .collect::<Result<_, _>>()?)
    }

    pub fn acknowledge_command(&mut self, id: Uuid) -> Result<()> {
        self.connection.execute(
            "UPDATE command_journal SET acknowledged=1 WHERE id=?1 AND result IS NOT NULL",
            [id.to_string()],
        )?;
        Ok(())
    }

    pub(super) fn queue_command(&mut self, command: &RemoteCommand) -> Result<()> {
        let tx = self.connection.transaction()?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT spec FROM command_journal WHERE id=?1",
                [command.id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(previous) = previous {
            ensure!(
                serde_json::from_str::<RemoteCommand>(&previous)? == *command,
                "command identifier changed content"
            );
            return Ok(());
        }
        ensure!(command.valid(), "invalid remote command");
        tx.execute("DELETE FROM command_journal WHERE acknowledged=1 AND CAST(json_extract(spec,'$.expires_at') AS INTEGER)<?1 AND id NOT IN (SELECT id FROM command_lifecycle WHERE started IS NOT NULL AND started_acknowledged=0)", [now_timestamp()-7*86400])?;
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM command_journal", [], |r| r.get(0))?;
        ensure!(count < 10_000, "command journal reached retention limit");
        tx.execute(
            "INSERT INTO command_journal(id,spec) VALUES(?1,?2)",
            params![command.id.to_string(), serde_json::to_string(command)?],
        )?;
        tx.execute(
            "INSERT INTO command_lifecycle(id,claim_id) VALUES(?1,?2)",
            params![command.id.to_string(), Uuid::new_v4().to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(super) fn pending_commands(&self) -> Result<Vec<CommandRecord>> {
        self.command_records(false)
    }

    pub(super) fn pending_command_cleanup(&self) -> Result<Vec<CommandRecord>> {
        self.command_records(true)
    }

    fn command_records(&self, cleanup_only: bool) -> Result<Vec<CommandRecord>> {
        let mut query = self.connection.prepare("SELECT j.spec,l.claim_id,l.process,l.cancel_requested,l.recovering FROM command_journal j JOIN command_lifecycle l USING(id) WHERE j.result IS NULL AND (?1=0 OR l.process IS NOT NULL) ORDER BY j.rowid LIMIT 64")?;
        let rows = query
            .query_map([cleanup_only], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, bool>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(spec, claim, process, cancel_requested, recovering)| {
                Ok(CommandRecord {
                    command: serde_json::from_str(&spec)?,
                    claim_id: Uuid::parse_str(&claim)?,
                    process: process
                        .map(|value| serde_json::from_str(&value))
                        .transpose()?,
                    cancel_requested,
                    recovering,
                })
            })
            .collect()
    }

    pub(super) fn command_spawned(
        &mut self,
        id: Uuid,
        process: &CommandProcessIdentity,
    ) -> Result<()> {
        let updated = self.connection.execute("UPDATE command_lifecycle SET process=?2 WHERE id=?1 AND process IS NULL AND recovering=0", params![id.to_string(), serde_json::to_string(process)?])?;
        ensure!(updated == 1, "command is no longer allowed to start");
        Ok(())
    }

    pub(super) fn command_started(&mut self, id: Uuid, started: &CommandStarted) -> Result<()> {
        let updated = self.connection.execute("UPDATE command_lifecycle SET started=?2 WHERE id=?1 AND started IS NULL AND process IS NOT NULL AND claim_id=?3", params![id.to_string(),serde_json::to_string(started)?,started.claim_id.to_string()])?;
        ensure!(
            updated == 1,
            "command start was already recorded or not claimed"
        );
        Ok(())
    }

    pub(super) fn command_starts(&self) -> Result<Vec<(Uuid, CommandStarted)>> {
        let mut query = self.connection.prepare("SELECT id,started FROM command_lifecycle WHERE started IS NOT NULL AND started_acknowledged=0 ORDER BY rowid LIMIT 64")?;
        let rows = query
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, started)| Ok((Uuid::parse_str(&id)?, serde_json::from_str(&started)?)))
            .collect()
    }

    pub(super) fn acknowledge_command_start(&mut self, id: Uuid) -> Result<()> {
        self.connection.execute("UPDATE command_lifecycle SET started_acknowledged=1 WHERE id=?1 AND started IS NOT NULL", [id.to_string()])?;
        Ok(())
    }

    pub(super) fn request_command_cancel(&mut self, id: Uuid) -> Result<()> {
        self.connection.execute(
            "UPDATE command_lifecycle SET cancel_requested=1 WHERE id=?1",
            [id.to_string()],
        )?;
        Ok(())
    }

    pub(super) fn defer_command_cleanup(&mut self, id: Uuid) -> Result<bool> {
        let count = self.connection.execute(
            "UPDATE command_lifecycle SET recovering=1 WHERE id=?1 AND process IS NOT NULL",
            [id.to_string()],
        )?;
        Ok(count == 1)
    }

    pub fn save_probe_result(&mut self, result: &ProbeResult) -> Result<()> {
        let now =
            now_millis().saturating_add(self.get_json::<i64>("clock_offset_ms")?.unwrap_or(0));
        let tx = self.connection.transaction()?;
        tx.execute(
            "INSERT INTO probe_outbox(id,sampled_at,result) VALUES(?1,?2,?3)",
            params![
                result.id.to_string(),
                result.sampled_at,
                serde_json::to_string(result)?
            ],
        )?;
        tx.execute(
            "DELETE FROM probe_outbox WHERE sampled_at<?1",
            [now.saturating_sub(2 * 3_600_000)],
        )?;
        tx.execute("DELETE FROM probe_outbox WHERE id IN (SELECT id FROM probe_outbox ORDER BY sampled_at DESC,rowid DESC LIMIT -1 OFFSET 4096)", [])?;
        tx.commit()?;
        Ok(())
    }

    pub fn probe_results(&self) -> Result<Vec<ProbeResult>> {
        let mut query = self
            .connection
            .prepare("SELECT result FROM probe_outbox ORDER BY sampled_at,rowid LIMIT 64")?;
        let rows = query
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .map(|v| serde_json::from_str(&v))
            .collect::<Result<_, _>>()?)
    }

    pub fn acknowledge_probes(&mut self, ids: &[Uuid]) -> Result<()> {
        ensure!(ids.len() <= 64, "too many probe acknowledgments");
        let tx = self.connection.transaction()?;
        for id in ids {
            tx.execute("DELETE FROM probe_outbox WHERE id=?1", [id.to_string()])?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

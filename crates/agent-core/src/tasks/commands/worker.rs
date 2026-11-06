use super::*;
use crate::state::Locked;

pub(super) async fn flush(state: &SharedState, client: &PanelClient) -> Result<()> {
    let mut failed = flush_starts(state, client, None).await.is_err();
    let results = state.locked()?.command_results()?;
    for result in results {
        let received: Result<TaskAck> = async {
            tokio::time::timeout(
                Duration::from_secs(3),
                client.post_json(&format!("/api/agent/v1/commands/{}", result.id), &result),
            )
            .await?
        }
        .await;
        match received {
            Ok(ack) if ack.ids == [result.id] => state.locked()?.acknowledge_command(result.id)?,
            _ => failed = true,
        }
    }
    ensure!(!failed, "some command result acknowledgments are pending");
    Ok(())
}

pub(super) async fn flush_starts(
    state: &SharedState,
    client: &PanelClient,
    only: Option<uuid::Uuid>,
) -> Result<()> {
    let starts = state.locked()?.command_starts()?;
    let mut failed = false;
    for (id, started) in starts
        .into_iter()
        .filter(|(id, _)| only.is_none_or(|only| only == *id))
    {
        let result: Result<TaskAck> = async {
            tokio::time::timeout(
                Duration::from_secs(3),
                client.post_json(&format!("/api/agent/v1/commands/{id}/started"), &started),
            )
            .await?
        }
        .await;
        match result {
            Ok(ack) if ack.ids == [id] => state.locked()?.acknowledge_command_start(id)?,
            _ => failed = true,
        }
    }
    ensure!(!failed, "some command status acknowledgments are pending");
    Ok(())
}

pub(super) async fn recover(state: &SharedState, ops: &dyn Privileged) -> Result<()> {
    let records = state.locked()?.pending_commands()?;
    for record in records {
        if record.recovering {
            cleanup_record(state, ops, record, false).await?;
        }
    }
    Ok(())
}

// The caller owns the retirement gate. This path never contacts the panel,
// releases a start gate, claims a command, or executes a stored shell payload.
pub(super) async fn recover_for_retirement(
    state: &SharedState,
    ops: &dyn Privileged,
) -> Result<()> {
    state.locked()?.recover_commands()?;
    loop {
        let records = state.locked()?.pending_command_cleanup()?;
        if records.is_empty() {
            return Ok(());
        }
        for record in records {
            cleanup_record(state, ops, record, true).await?;
        }
    }
}

async fn cleanup_record(
    state: &SharedState,
    ops: &dyn Privileged,
    record: crate::tasks::state::CommandRecord,
    retiring: bool,
) -> Result<()> {
    let Some(process) = record.process else {
        return Ok(());
    };
    ops.recover_command(&process).await?;
    let status = if record.cancel_requested && cfg!(unix) {
        CommandStatus::Cancelled
    } else {
        CommandStatus::Interrupted
    };
    let message = match (retiring, cfg!(unix)) {
        (true, true) => {
            "Device retirement; the managed process group was cleaned up and the command was not executed again"
        }
        (true, false) => {
            "Device retirement; the command was not executed again; detached descendant cleanup is not confirmed on this platform"
        }
        (false, true) => {
            "Agent restarted; the managed process group was cleaned up and the command was not executed again"
        }
        (false, false) => {
            "Agent restarted; the command was not executed again; detached descendant cleanup is not confirmed on this platform"
        }
    };
    let result = terminal(record.command.id, status, clock_offset(state)?, message);
    state.locked()?.finish_command(&result)?;
    Ok(())
}

pub(super) async fn tick(
    state: &SharedState,
    ops: &dyn Privileged,
    clients: &watch::Receiver<Option<Arc<PanelClient>>>,
    retirement: &crate::retirement::Retirement,
) -> Result<()> {
    let Some(client) = clients.borrow().clone() else {
        return Ok(());
    };
    if let Err(error) = flush(state, &client).await {
        tracing::debug!(%error,"durable command reports await acknowledgment");
    }
    if retirement.requested() {
        return Ok(());
    }
    let commands: Vec<RemoteCommand> = client.get_json("/api/agent/v1/commands/lifecycle").await?;
    ensure!(
        commands.len() <= 64 && commands.iter().all(RemoteCommand::valid),
        "invalid commands from panel"
    );
    for command in commands {
        state.locked()?.queue_command(&command)?;
    }
    let records = state.locked()?.pending_commands()?;
    for record in records {
        if retirement.requested() {
            break;
        }
        // A failed recovery is a local execution barrier, not permission to run
        // more commands while the old managed group may still be alive.
        ensure!(
            record.process.is_none(),
            "a previous command still requires cleanup"
        );
        let control: CommandControl = client
            .post_json(
                &format!("/api/agent/v1/commands/{}/claim", record.command.id),
                &CommandClaim {
                    claim_id: record.claim_id,
                },
            )
            .await?;
        ensure!(
            control.id == record.command.id,
            "invalid command claim acknowledgment"
        );
        let offset = clock_offset(state)?;
        let result = match control.state {
            CommandState::Cancelled | CommandState::Expired => {
                let status = if control.state == CommandState::Cancelled {
                    CommandStatus::Cancelled
                } else {
                    CommandStatus::Expired
                };
                let mut result = terminal(record.command.id, status, offset, "");
                result.finished_at = control.finished_at.unwrap_or(result.finished_at);
                result
            }
            CommandState::CancelRequested => {
                state.locked()?.request_command_cancel(record.command.id)?;
                terminal(
                    record.command.id,
                    CommandStatus::Cancelled,
                    offset,
                    "Command cancelled before execution started",
                )
            }
            CommandState::Claimed if record.recovering => terminal(
                record.command.id,
                CommandStatus::Interrupted,
                offset,
                "Agent restarted before execution; command was not executed again",
            ),
            CommandState::Claimed if record.command.expires_at <= now_timestamp() + offset => {
                terminal(record.command.id, CommandStatus::Expired, offset, "")
            }
            CommandState::Claimed => {
                let observer = JournalObserver {
                    state,
                    command: &record.command,
                    claim_id: record.claim_id,
                    offset,
                    minimum_start: control.claimed_at.unwrap_or(0),
                    cancel: AtomicBool::new(false),
                    retirement,
                };
                execution_result(
                    state,
                    record.command.id,
                    offset,
                    execute(ops, &observer, clients).await,
                )?
            }
            _ => anyhow::bail!("command state does not allow a new execution"),
        };
        state.locked()?.finish_command(&result)?;
        if let Err(error) = flush(state, &client).await {
            tracing::debug!(%error,"command result will retry after reconnect");
        }
    }
    Ok(())
}

pub(super) fn execution_result(
    state: &SharedState,
    id: uuid::Uuid,
    offset: i64,
    result: Result<CommandResult>,
) -> Result<CommandResult> {
    match result {
        Ok(result) => Ok(result),
        Err(error) => {
            if state.locked()?.defer_command_cleanup(id)? {
                anyhow::bail!("command execution ended without confirmed cleanup: {error}");
            }
            Ok(terminal(
                id,
                CommandStatus::Failed,
                offset,
                &error.to_string().chars().take(1024).collect::<String>(),
            ))
        }
    }
}

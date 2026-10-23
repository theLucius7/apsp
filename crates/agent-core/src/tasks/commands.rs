use crate::{SharedState, artifacts::PanelClient};
use anyhow::{Result, ensure};
use apsp_adapter_sdk::{CommandObserver, CommandProcessIdentity, Privileged};
use apsp_protocol::{
    CommandClaim, CommandControl, CommandResult, CommandStarted, CommandState, CommandStatus,
    RemoteCommand, TaskAck, now_timestamp,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;

mod worker;
use worker::{recover, tick};

pub(super) async fn run(
    allow_remote_commands: bool,
    state: SharedState,
    ops: Arc<dyn Privileged>,
    clients: watch::Receiver<Option<Arc<PanelClient>>>,
    retirement: Arc<crate::retirement::Retirement>,
) -> Result<()> {
    // The permission comes only from local configuration, never from the panel.
    if !allow_remote_commands {
        return Ok(());
    }
    {
        let _guard = retirement.gate.read().await;
        if !retirement.requested() {
            state
                .lock()
                .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
                .recover_commands()?;
        }
    }
    loop {
        let _guard = retirement.gate.read().await;
        if retirement.requested() {
            if let Err(error) = worker::recover_for_retirement(&state, ops.as_ref()).await {
                tracing::warn!(%error,"retirement awaits confirmed command cleanup");
            }
        } else {
            if let Err(error) = recover(&state, ops.as_ref()).await {
                tracing::warn!(%error,"command recovery awaits confirmed cleanup");
            }
            if let Err(error) = tick(&state, ops.as_ref(), &clients, &retirement).await {
                tracing::warn!(%error,"command worker will retry durable state");
            }
        }
        drop(_guard);
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

fn terminal(id: uuid::Uuid, status: CommandStatus, offset: i64, message: &str) -> CommandResult {
    CommandResult {
        id,
        status,
        finished_at: now_timestamp() + offset,
        stdout: String::new(),
        stderr: message.into(),
        timed_out: false,
        truncated: false,
    }
}

fn clock_offset(state: &SharedState) -> Result<i64> {
    Ok(state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
        .get_json::<i64>("clock_offset_ms")?
        .unwrap_or(0)
        / 1000)
}

struct JournalObserver<'a> {
    state: &'a SharedState,
    command: &'a RemoteCommand,
    claim_id: uuid::Uuid,
    offset: i64,
    minimum_start: i64,
    cancel: AtomicBool,
    retirement: &'a crate::retirement::Retirement,
}

impl CommandObserver for JournalObserver<'_> {
    fn spawned(&self, process: &CommandProcessIdentity) -> Result<()> {
        ensure!(
            !self.retirement.requested(),
            "device retirement prevents command execution"
        );
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
            .command_spawned(self.command.id, process)
    }
    fn started(&self) -> Result<()> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
            .command_started(
                self.command.id,
                &CommandStarted {
                    claim_id: self.claim_id,
                    started_at: (now_timestamp() + self.offset).max(self.minimum_start),
                },
            )
    }
    fn cancellation_requested(&self) -> bool {
        self.cancel.load(Ordering::SeqCst) || self.retirement.requested()
    }
}

async fn execute(
    ops: &dyn Privileged,
    observer: &JournalObserver<'_>,
    clients: &watch::Receiver<Option<Arc<PanelClient>>>,
) -> Result<CommandResult> {
    let command = observer.command;
    #[cfg(unix)]
    let (program, args) = (
        Path::new("/bin/sh"),
        vec!["-c".into(), command.command.clone()],
    );
    #[cfg(windows)]
    let (program, args) = (
        Path::new("powershell.exe"),
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            command.command.clone(),
        ],
    );
    let operation =
        ops.execute_controlled(program, &args, command.timeout_secs, 256 * 1024, observer);
    let monitoring = async {
        loop {
            let client = clients.borrow().clone();
            if let Some(client) = client {
                let status = tokio::time::timeout(
                    Duration::from_secs(3),
                    client.get_json::<CommandControl>(&format!(
                        "/api/agent/v1/commands/{}/control",
                        command.id
                    )),
                )
                .await;
                if let Ok(Ok(control)) = status
                    && control.id == command.id
                    && control.state == CommandState::CancelRequested
                {
                    let saved = observer
                        .state
                        .lock()
                        .map_err(|_| anyhow::anyhow!("state lock poisoned"))
                        .and_then(|mut state| state.request_command_cancel(command.id));
                    match saved {
                        Ok(()) => observer.cancel.store(true, Ordering::SeqCst),
                        Err(error) => tracing::warn!(%error,"cannot persist command cancellation"),
                    }
                }
                if let Err(error) =
                    worker::flush_starts(observer.state, &client, Some(command.id)).await
                {
                    tracing::debug!(%error,"command status upload will retry");
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    };
    let output = tokio::select! { result=operation => result, ()=monitoring => unreachable!() };
    let mut result = terminal(command.id, CommandStatus::Failed, observer.offset, "");
    match output {
        Ok(output) => {
            result.status = if output.cancelled {
                if observer.cancel.load(Ordering::SeqCst) {
                    CommandStatus::Cancelled
                } else {
                    CommandStatus::Interrupted
                }
            } else if output.execution.output.success && !output.execution.timed_out {
                CommandStatus::Succeeded
            } else {
                CommandStatus::Failed
            };
            result.stdout = output.execution.output.stdout;
            result.stderr = output.execution.output.stderr;
            result.timed_out = output.execution.timed_out;
            result.truncated = output.execution.truncated;
        }
        Err(error) => return Err(error),
    }
    result.finished_at = result.finished_at.max(observer.minimum_start);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Config, State, fake::FakeServiceManager, retirement::Retirement, system::SystemOps,
    };
    use std::sync::Mutex;
    use uuid::Uuid;

    #[test]
    fn execution_errors_keep_cleanup_pending_even_without_a_cancel_request() -> Result<()> {
        let state = Arc::new(Mutex::new(State::open(Path::new(":memory:"))?));
        let command = RemoteCommand {
            id: Uuid::new_v4(),
            command: "echo fixture".into(),
            timeout_secs: 1,
            expires_at: now_timestamp() + 30,
        };
        state.lock().unwrap().queue_command(&command)?;
        state.lock().unwrap().command_spawned(
            command.id,
            &CommandProcessIdentity {
                pid: 12345,
                started: "TEST_ONLY_process".into(),
            },
        )?;
        let failure = terminal(
            command.id,
            CommandStatus::Failed,
            0,
            "ordinary nonzero exit with confirmed cleanup",
        );
        assert!(worker::execution_result(&state, command.id, 0, Ok(failure)).is_ok());
        assert!(!state.lock().unwrap().pending_commands()?[0].recovering);
        assert!(
            worker::execution_result(
                &state,
                command.id,
                0,
                Err(anyhow::anyhow!("process group cleanup failed"))
            )
            .is_err()
        );
        let records = state.lock().unwrap().pending_commands()?;
        assert!(records[0].recovering);
        assert!(!records[0].cancel_requested);
        assert!(state.lock().unwrap().command_results()?.is_empty());
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn started_execution_survives_reopen_as_interrupted_and_is_never_reexecuted() -> Result<()>
    {
        let directory =
            std::env::temp_dir().join(format!("apsp-command-restart-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory)?;
        let root = std::fs::canonicalize(&directory)?;
        let config = Config {
            state_db: root.join("state.db"),
            runtime_root: root.join("runtime"),
            install_root: root.join("install"),
            ..Config::default()
        };
        let state = Arc::new(Mutex::new(State::open(&config.state_db)?));
        let ops = Arc::new(SystemOps);
        let retirement = Arc::new(Retirement::new(
            config.clone(),
            state.clone(),
            Vec::new(),
            ops.clone(),
            Arc::new(FakeServiceManager::default()),
        )?);
        let command = RemoteCommand {
            id: Uuid::new_v4(),
            command: "sleep 30 & wait".into(),
            timeout_secs: 30,
            expires_at: now_timestamp() + 60,
        };
        state.lock().unwrap().queue_command(&command)?;
        let claim_id = state.lock().unwrap().pending_commands()?[0].claim_id;
        let (_sender, clients) = watch::channel(None);
        {
            let observer = JournalObserver {
                state: &state,
                command: &command,
                claim_id,
                offset: 0,
                minimum_start: now_timestamp(),
                cancel: AtomicBool::new(false),
                retirement: &retirement,
            };
            let execution = execute(ops.as_ref(), &observer, &clients);
            tokio::pin!(execution);
            tokio::select! {
                result=&mut execution => panic!("command ended before restart fixture: {result:?}"),
                ready=tokio::time::timeout(Duration::from_secs(5),async {
                    loop {
                        if !state.lock().unwrap().command_starts()?.is_empty() { break Ok::<_,anyhow::Error>(()); }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }) => ready??,
            }
        }
        *state.lock().unwrap() = State::open(&config.state_db)?;
        state.lock().unwrap().recover_commands()?;
        recover(&state, ops.as_ref()).await?;
        let results = state.lock().unwrap().command_results()?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, CommandStatus::Interrupted);
        assert!(state.lock().unwrap().pending_commands()?.is_empty());
        assert_eq!(state.lock().unwrap().command_starts()?.len(), 1);
        state.lock().unwrap().queue_command(&command)?;
        assert!(state.lock().unwrap().pending_commands()?.is_empty());
        drop(retirement);
        drop(state);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[tokio::test]
    async fn disabled_commands_do_not_contact_panel_or_touch_pending_execution() -> Result<()> {
        let directory =
            std::env::temp_dir().join(format!("apsp-command-opt-in-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory)?;
        let root = std::fs::canonicalize(&directory)?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let config = Config {
            panel_url: format!("http://{}", listener.local_addr()?),
            state_db: root.join("state.db"),
            runtime_root: root.join("runtime"),
            install_root: root.join("install"),
            ..Config::default()
        };
        let state = Arc::new(Mutex::new(State::open(&config.state_db)?));
        let command = RemoteCommand {
            id: Uuid::new_v4(),
            command: "echo should-not-execute".into(),
            timeout_secs: 1,
            expires_at: now_timestamp() + 60,
        };
        state.lock().unwrap().begin_command(&command)?;
        let ops = Arc::new(SystemOps);
        let retirement = Arc::new(Retirement::new(
            config.clone(),
            state.clone(),
            Vec::new(),
            ops.clone(),
            Arc::new(FakeServiceManager::default()),
        )?);
        let (_sender, receiver) = watch::channel(Some(Arc::new(PanelClient::new(
            &config.panel_url,
            "TEST_ONLY_session",
        )?)));
        tokio::time::timeout(
            Duration::from_secs(1),
            run(
                config.allow_remote_commands,
                state.clone(),
                ops,
                receiver,
                retirement,
            ),
        )
        .await??;
        assert!(
            tokio::time::timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
        // Even recovery of earlier work is untouched while the local permission is off.
        assert!(state.lock().unwrap().command_results()?.is_empty());
        assert!(!state.lock().unwrap().begin_command(&command)?);
        drop(state);
        std::fs::remove_dir_all(&root)?;
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod retirement_tests;

pub(super) async fn cleanup_for_retirement(
    state: &SharedState,
    ops: &dyn Privileged,
) -> Result<()> {
    worker::recover_for_retirement(state, ops).await
}

use super::*;
use crate::{
    Config, State, fake::FakeServiceManager, identity, retirement::Retirement, system::SystemOps,
};
use apsp_adapter_sdk::{BoxFuture, CommandOutput, ControlledExecution};
use apsp_protocol::RetirementRequest;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{Mutex, atomic::AtomicUsize},
};
use tokio::time::timeout;
use uuid::Uuid;

struct CleanupOps {
    inner: SystemOps,
    fail_recovery: AtomicBool,
    attempts: AtomicUsize,
    payload_executions: AtomicUsize,
}
impl Privileged for CleanupOps {
    fn recover_command<'a>(&'a self, process: &'a CommandProcessIdentity) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            ensure!(
                !self.fail_recovery.load(Ordering::SeqCst),
                "injected unconfirmed process group cleanup"
            );
            self.inner.recover_command(process).await
        })
    }
    fn execute_controlled<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
        seconds: u32,
        maximum: usize,
        observer: &'a dyn CommandObserver,
    ) -> BoxFuture<'a, ControlledExecution> {
        self.payload_executions.fetch_add(1, Ordering::SeqCst);
        self.inner
            .execute_controlled(program, args, seconds, maximum, observer)
    }
    fn execute<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
    ) -> BoxFuture<'a, CommandOutput> {
        self.inner.execute(program, args)
    }
    fn create_dir<'a>(
        &'a self,
        path: &'a Path,
        mode: u32,
        group: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        self.inner.create_dir(path, mode, group)
    }
    fn write_file<'a>(
        &'a self,
        path: &'a Path,
        bytes: &'a [u8],
        mode: u32,
        group: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        self.inner.write_file(path, bytes, mode, group)
    }
    fn atomic_symlink<'a>(&'a self, link: &'a Path, target: &'a Path) -> BoxFuture<'a, ()> {
        self.inner.atomic_symlink(link, target)
    }
    fn remove_symlink<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, ()> {
        self.inner.remove_symlink(path)
    }
    fn remove_file<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, ()> {
        self.inner.remove_file(path)
    }
    fn remove_managed_directory<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, ()> {
        self.inner.remove_managed_directory(path)
    }
    fn install_archive<'a>(
        &'a self,
        archive: &'a Path,
        directory: &'a Path,
        name: &'a str,
    ) -> BoxFuture<'a, ()> {
        self.inner.install_archive(archive, directory, name)
    }
}

struct LiveFixture {
    root: PathBuf,
    config: Config,
    state: SharedState,
    ops: Arc<CleanupOps>,
    retirement: Arc<Retirement>,
    identity: identity::Identity,
    execution: tokio::task::JoinHandle<Result<CommandResult>>,
}
impl Drop for LiveFixture {
    fn drop(&mut self) {
        self.execution.abort();
        let _ = fs::remove_dir_all(&self.root);
    }
}
impl LiveFixture {
    async fn start() -> Result<Self> {
        let root = std::env::temp_dir().join(format!("apsp-retired-command-{}", Uuid::new_v4()));
        fs::create_dir(&root)?;
        let root = fs::canonicalize(root)?;
        let config = Config {
            panel_url: "http://127.0.0.1:9".into(),
            identity_dir: root.join("identity"),
            state_db: root.join("state.db"),
            runtime_root: root.join("runtime"),
            install_root: root.join("install"),
            ..Config::default()
        };
        fs::create_dir(&config.identity_dir)?;
        fs::write(config.identity_dir.join("device.key"), [19_u8; 32])?;
        fs::set_permissions(
            config.identity_dir.join("device.key"),
            fs::Permissions::from_mode(0o600),
        )?;
        fs::write(config.identity_dir.join("server_id"), "7")?;
        fs::write(config.identity_dir.join("panel_origin"), &config.panel_url)?;
        let identity = identity::load(&config)?;
        let state = Arc::new(Mutex::new(State::open(&config.state_db)?));
        let ops = Arc::new(CleanupOps {
            inner: SystemOps,
            fail_recovery: AtomicBool::new(true),
            attempts: AtomicUsize::new(0),
            payload_executions: AtomicUsize::new(0),
        });
        // The old actor's private cancellation flag models an orphaned managed
        // group after restart; the new retirement object cannot toggle that flag.
        let old = Arc::new(Retirement::new(
            config.clone(),
            state.clone(),
            vec![],
            ops.clone(),
            Arc::new(FakeServiceManager::default()),
        )?);
        let marker = root
            .join("payload-count")
            .to_string_lossy()
            .replace('\'', "'\"'\"'");
        let command = RemoteCommand {
            id: Uuid::new_v4(),
            command: format!("printf x >> '{marker}'; sleep 30 & wait"),
            timeout_secs: 30,
            expires_at: now_timestamp() + 60,
        };
        state.lock().unwrap().queue_command(&command)?;
        let claim = state.lock().unwrap().pending_commands()?.remove(0).claim_id;
        let (task_state, task_ops) = (state.clone(), ops.clone());
        let execution = tokio::spawn(async move {
            let (_sender, clients) = watch::channel(None);
            let observer = JournalObserver {
                state: &task_state,
                command: &command,
                claim_id: claim,
                offset: 0,
                minimum_start: now_timestamp(),
                cancel: AtomicBool::new(false),
                retirement: &old,
            };
            execute(task_ops.as_ref(), &observer, &clients).await
        });
        timeout(Duration::from_secs(5), async {
            loop {
                if !state.lock().unwrap().command_starts()?.is_empty()
                    && root.join("payload-count").exists()
                {
                    break Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        let retirement = Arc::new(Retirement::new(
            config.clone(),
            state.clone(),
            vec![],
            ops.clone(),
            Arc::new(FakeServiceManager::default()),
        )?);
        retirement.request(
            &identity,
            RetirementRequest {
                request_id: Uuid::new_v4(),
            },
        )?;
        Ok(Self {
            root,
            config,
            state,
            ops,
            retirement,
            identity,
            execution,
        })
    }
    fn assert_unconfirmed(&self) -> Result<()> {
        assert!(self.config.identity_dir.join("device.key").exists());
        assert!(self.state.lock().unwrap().command_results()?.is_empty());
        assert!(
            !self
                .state
                .lock()
                .unwrap()
                .pending_command_cleanup()?
                .is_empty()
        );
        assert_eq!(fs::read(self.root.join("payload-count"))?, b"x");
        assert_eq!(self.ops.payload_executions.load(Ordering::SeqCst), 1);
        Ok(())
    }
}

#[tokio::test]
async fn retirement_prepare_and_completion_await_real_registered_group_cleanup() -> Result<()> {
    let fixture = LiveFixture::start().await?;
    assert!(!fixture.config.allow_remote_commands);
    let starts = fixture.state.lock().unwrap().command_starts()?;
    assert!(fixture.retirement.prepare().await.is_err());
    fixture.assert_unconfirmed()?;
    assert!(
        fixture
            .retirement
            .complete(&fixture.identity)
            .await
            .is_err()
    );
    fixture.assert_unconfirmed()?;
    fixture.ops.fail_recovery.store(false, Ordering::SeqCst);
    timeout(Duration::from_secs(10), fixture.retirement.prepare()).await??;
    let results = fixture.state.lock().unwrap().command_results()?;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, CommandStatus::Interrupted);
    assert_eq!(fixture.state.lock().unwrap().command_starts()?, starts);
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .pending_command_cleanup()?
            .is_empty()
    );
    assert_eq!(fs::read(fixture.root.join("payload-count"))?, b"x");
    assert_eq!(fixture.ops.payload_executions.load(Ordering::SeqCst), 1);
    fixture.retirement.complete(&fixture.identity).await?;
    assert!(!fixture.config.identity_dir.join("device.key").exists());
    Ok(())
}

#[tokio::test]
async fn resumed_stopped_retirement_cannot_clear_credentials_with_unconfirmed_commands()
-> Result<()> {
    let fixture = LiveFixture::start().await?;
    let mut record: serde_json::Value = fixture
        .state
        .lock()
        .unwrap()
        .get_json("retirement")?
        .unwrap();
    record["phase"] = serde_json::json!("stopped");
    fixture
        .state
        .lock()
        .unwrap()
        .set_json("retirement", &record)?;
    assert!(
        fixture
            .retirement
            .complete(&fixture.identity)
            .await
            .is_err()
    );
    fixture.assert_unconfirmed()?;
    assert!(fixture.retirement.recover_completion().await.is_err());
    fixture.assert_unconfirmed()?;
    fixture.ops.fail_recovery.store(false, Ordering::SeqCst);
    timeout(
        Duration::from_secs(10),
        fixture.retirement.complete(&fixture.identity),
    )
    .await??;
    assert!(!fixture.config.identity_dir.join("device.key").exists());
    assert_eq!(fs::read(fixture.root.join("payload-count"))?, b"x");
    assert_eq!(fixture.ops.payload_executions.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn requested_retirement_worker_retries_cleanup_without_any_panel_or_payload_work()
-> Result<()> {
    let fixture = LiveFixture::start().await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let requests = Arc::new(AtomicUsize::new(0));
    let captured = requests.clone();
    let endpoint = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move {
        while let Ok((_stream, _address)) = listener.accept().await {
            captured.fetch_add(1, Ordering::SeqCst);
        }
    });
    let client = Arc::new(PanelClient::new(&endpoint, "TEST_ONLY_retiring_command")?);
    let (_sender, clients) = watch::channel(Some(client));
    let worker = tokio::spawn(run(
        true,
        fixture.state.clone(),
        fixture.ops.clone(),
        clients,
        fixture.retirement.clone(),
    ));
    timeout(Duration::from_secs(5), async {
        while fixture.ops.attempts.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    fixture.assert_unconfirmed()?;
    fixture.ops.fail_recovery.store(false, Ordering::SeqCst);
    timeout(Duration::from_secs(10), async {
        loop {
            if fixture
                .state
                .lock()
                .unwrap()
                .pending_command_cleanup()?
                .is_empty()
            {
                break Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    let results = fixture.state.lock().unwrap().command_results()?;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, CommandStatus::Interrupted);
    assert_eq!(fixture.ops.payload_executions.load(Ordering::SeqCst), 1);
    assert_eq!(fs::read(fixture.root.join("payload-count"))?, b"x");
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    server.abort();
    Ok(())
}

#[tokio::test]
async fn resumed_clearing_phase_cannot_delete_identity_before_confirmed_cleanup() -> Result<()> {
    let fixture = LiveFixture::start().await?;
    let mut record: serde_json::Value = fixture
        .state
        .lock()
        .unwrap()
        .get_json("retirement")?
        .unwrap();
    record["phase"] = serde_json::json!("clearing");
    record["receipt"] = serde_json::json!({"server_id":7,"request_id":record["request_id"].clone(),"signature":"TEST_ONLY_UNDELIVERED_RECEIPT"});
    fixture
        .state
        .lock()
        .unwrap()
        .set_json("retirement", &record)?;
    assert!(fixture.retirement.recover_completion().await.is_err());
    fixture.assert_unconfirmed()?;
    let preserved: serde_json::Value = fixture
        .state
        .lock()
        .unwrap()
        .get_json("retirement")?
        .unwrap();
    assert_eq!(preserved, record);
    Ok(())
}

#[test]
fn retirement_cleanup_finds_registered_group_after_unstarted_queue() -> Result<()> {
    let mut state = State::open(Path::new(":memory:"))?;
    for _ in 0..64 {
        state.queue_command(&RemoteCommand {
            id: Uuid::new_v4(),
            command: "printf TEST_ONLY_queued".into(),
            timeout_secs: 1,
            expires_at: now_timestamp() + 60,
        })?;
    }
    let tracked = RemoteCommand {
        id: Uuid::new_v4(),
        command: "printf TEST_ONLY_tracked".into(),
        timeout_secs: 1,
        expires_at: now_timestamp() + 60,
    };
    state.queue_command(&tracked)?;
    state.command_spawned(
        tracked.id,
        &CommandProcessIdentity {
            pid: 12345,
            started: "TEST_ONLY_no_process_created".into(),
        },
    )?;
    state.request_command_cancel(tracked.id)?;
    assert_eq!(state.pending_commands()?.len(), 64);
    assert!(
        state
            .pending_commands()?
            .iter()
            .all(|record| record.process.is_none())
    );
    let cleanup = state.pending_command_cleanup()?;
    assert_eq!(cleanup.len(), 1);
    assert_eq!(cleanup[0].command.id, tracked.id);
    assert!(cleanup[0].cancel_requested);
    Ok(())
}

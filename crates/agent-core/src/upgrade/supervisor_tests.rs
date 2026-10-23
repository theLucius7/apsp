use super::*;
use crate::{
    release_test_support::{entry, hash, install_proof, signed_release},
    system::SystemOps,
};
use apsp_adapter_sdk::{BoxFuture, CommandOutput};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

static SYSTEM: SystemOps = SystemOps;

#[derive(Clone, Copy)]
enum RetireAt {
    CachePreflight,
    CurrentShutdown,
    CandidateStartup,
}

struct Process {
    retired: Arc<AtomicBool>,
    terminations: Arc<AtomicUsize>,
    retire_on_termination: bool,
}

impl ManagedProcess for Process {
    fn id(&self) -> u32 {
        42
    }
    fn try_wait(&mut self) -> Result<Option<bool>> {
        Ok(self.retired.load(Ordering::SeqCst).then_some(false))
    }
    fn exit_code(&self) -> Option<i32> {
        self.retired
            .load(Ordering::SeqCst)
            .then_some(crate::retirement::RETIRED_EXIT_CODE)
    }
    fn terminate(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.terminations.fetch_add(1, Ordering::SeqCst);
            if self.retire_on_termination {
                self.retired.store(true, Ordering::SeqCst);
            }
            Ok(())
        })
    }
}

struct Ops {
    at: RetireAt,
    retired: Arc<AtomicBool>,
    spawns: AtomicUsize,
    terminations: Arc<AtomicUsize>,
}

impl Privileged for Ops {
    fn spawn_managed<'a>(
        &'a self,
        program: &'a Path,
        _: &'a [String],
    ) -> BoxFuture<'a, Box<dyn ManagedProcess>> {
        Box::pin(async move {
            self.spawns.fetch_add(1, Ordering::SeqCst);
            let candidate = program
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|version| version == "99.0.0");
            let retired = if candidate && matches!(self.at, RetireAt::CandidateStartup) {
                Arc::new(AtomicBool::new(true))
            } else {
                self.retired.clone()
            };
            Ok(Box::new(Process {
                retired,
                terminations: self.terminations.clone(),
                retire_on_termination: !candidate && matches!(self.at, RetireAt::CurrentShutdown),
            }) as Box<dyn ManagedProcess>)
        })
    }

    fn execute<'a>(&'a self, _: &'a Path, args: &'a [String]) -> BoxFuture<'a, CommandOutput> {
        Box::pin(async move {
            let stdout = if args == ["--version"] {
                "apsp-agent 99.0.0\n".into()
            } else {
                assert_eq!(args.last().map(String::as_str), Some("verify-cache"));
                if matches!(self.at, RetireAt::CachePreflight) {
                    self.retired.store(true, Ordering::SeqCst);
                }
                String::new()
            };
            Ok(CommandOutput {
                success: true,
                stdout,
                stderr: String::new(),
            })
        })
    }

    fn create_dir<'a>(
        &'a self,
        path: &'a Path,
        mode: u32,
        group: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        SYSTEM.create_dir(path, mode, group)
    }
    fn write_file<'a>(
        &'a self,
        path: &'a Path,
        bytes: &'a [u8],
        mode: u32,
        group: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        SYSTEM.write_file(path, bytes, mode, group)
    }
    fn atomic_symlink<'a>(&'a self, link: &'a Path, target: &'a Path) -> BoxFuture<'a, ()> {
        SYSTEM.atomic_symlink(link, target)
    }
    fn remove_symlink<'a>(&'a self, link: &'a Path) -> BoxFuture<'a, ()> {
        SYSTEM.remove_symlink(link)
    }
    fn install_archive<'a>(&'a self, _: &'a Path, _: &'a Path, _: &'a str) -> BoxFuture<'a, ()> {
        panic!("supervision must not install runtime archives")
    }
}

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn assert_terminal_retirement(at: RetireAt, expected_spawns: usize) -> Result<()> {
    let temporary =
        std::env::temp_dir().join(format!("apsp-supervisor-retired-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&temporary)?;
    let fixture = Fixture(std::fs::canonicalize(temporary)?);
    let config = Config {
        panel_url: "http://127.0.0.1:9".into(),
        agent_root: fixture.0.join("core"),
        state_db: fixture.0.join("state/state.db"),
        status_socket: fixture.0.join("status.sock"),
        ..Config::default()
    };
    let path = fixture.0.join("agent.toml");
    std::fs::write(&path, toml::to_string(&config)?)?;
    // Signed, native-format bytes pass the real trust gates. Mocked process
    // execution controls the retirement instant without launching this payload.
    let mut bytes = vec![0_u8; 32];
    if cfg!(target_os = "macos") {
        bytes[..4].copy_from_slice(b"\xcf\xfa\xed\xfe");
        bytes[4..8].copy_from_slice(&0x100000c_u32.to_le_bytes());
    } else {
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        let machine: u16 = if cfg!(target_arch = "aarch64") {
            183
        } else {
            62
        };
        bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    }
    let proof = signed_release(
        ["1.0.0", "99.0.0"]
            .into_iter()
            .map(|version| {
                (
                    entry("agent", version, executable_name(), "raw", &bytes, &bytes),
                    bytes.clone(),
                )
            })
            .collect(),
    );
    for version in ["1.0.0", "99.0.0"] {
        let directory = config.agent_root.join(version);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(directory.join(executable_name()), &bytes)?;
        install_proof(&directory, &proof);
    }
    std::os::unix::fs::symlink(
        config.agent_root.join("1.0.0"),
        config.agent_root.join("current"),
    )?;
    let pending = PendingUpgrade {
        version: "99.0.0".into(),
        sha256: hash(&bytes),
        proof: Some(proof),
    };
    let pending_path = config.agent_root.join("pending-update.json");
    let pending_bytes = serde_json::to_vec(&pending)?;
    std::fs::write(&pending_path, &pending_bytes)?;
    let ops = Arc::new(Ops {
        at,
        retired: Arc::new(AtomicBool::new(false)),
        spawns: AtomicUsize::new(0),
        terminations: Arc::new(AtomicUsize::new(0)),
    });
    let error = tokio::time::timeout(
        Duration::from_secs(8),
        supervise(config.clone(), path, false, ops.clone()),
    )
    .await?
    .unwrap_err();
    assert!(error.downcast_ref::<crate::retirement::Retired>().is_some());
    assert_eq!(ops.spawns.load(Ordering::SeqCst), expected_spawns);
    let state: UpgradeState = read_json(&config.agent_root.join("update-state.json"))?;
    assert_eq!(state.current, "1.0.0");
    assert!(state.failed_versions.is_empty());
    assert!(state.last_error.is_none());
    assert_eq!(std::fs::read(pending_path)?, pending_bytes);
    assert_eq!(
        reference(&config.agent_root.join("current"))?,
        config.agent_root.join("1.0.0")
    );
    if matches!(at, RetireAt::CachePreflight) {
        assert!(state.trial.is_none());
        assert_eq!(ops.terminations.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn retirement_during_candidate_cache_validation_prevents_trial_and_spawn() -> Result<()> {
    assert_terminal_retirement(RetireAt::CachePreflight, 1).await
}

#[tokio::test]
async fn candidate_retirement_during_startup_is_terminal_without_rollback() -> Result<()> {
    assert_terminal_retirement(RetireAt::CandidateStartup, 2).await
}

#[tokio::test]
async fn retirement_during_current_shutdown_prevents_candidate_spawn() -> Result<()> {
    assert_terminal_retirement(RetireAt::CurrentShutdown, 1).await
}

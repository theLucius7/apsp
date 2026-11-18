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

#[tokio::test]
async fn ready_requires_explicit_bundle_ready_and_aborts_on_bundle_error() -> Result<()> {
    use tokio::{io::AsyncWriteExt, net::UnixListener};
    let temporary =
        std::env::temp_dir().join(format!("apsp-supervisor-ready-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&temporary)?;
    let fixture = Fixture(std::fs::canonicalize(temporary)?);
    let socket_path = fixture.0.join("status.sock");
    let config = Config {
        status_socket: socket_path.clone(),
        ..Config::default()
    };
    let listener = UnixListener::bind(&socket_path)?;
    let payload = Arc::new(std::sync::Mutex::new(serde_json::json!({
        "agent_version": "0.4.0",
        "pid": 42
    })));
    let served = payload.clone();
    let server = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let body = serde_json::to_vec(&*served.lock().unwrap()).unwrap();
            let _ = stream.write_all(&body).await;
        }
    });
    let mut proc = Process {
        retired: Arc::new(AtomicBool::new(false)),
        terminations: Arc::new(AtomicUsize::new(0)),
        retire_on_termination: false,
    };
    // Missing `bundle_ready` must never default to `true`.
    assert!(
        tokio::time::timeout(
            Duration::from_millis(3500),
            ready(&mut proc, &config, "0.4.0"),
        )
        .await
        .is_err()
    );

    // A candidate reporting `bundle_error` while withholding `(agent_version, pid)` fails fast.
    *payload.lock().unwrap() = serde_json::json!({
        "agent_version": null,
        "pid": null,
        "bundle_ready": false,
        "bundle_error": "runtime health check failed"
    });
    let err = tokio::time::timeout(Duration::from_secs(3), ready(&mut proc, &config, "0.4.0"))
        .await?
        .unwrap_err();
    assert!(err.to_string().contains("runtime health check failed"));

    // Explicit `bundle_ready: true` with matching `(agent_version, pid)` passes after 3 polls.
    *payload.lock().unwrap() = serde_json::json!({
        "agent_version": "0.4.0",
        "pid": 42,
        "bundle_ready": true,
        "bundle_error": null
    });
    tokio::time::timeout(Duration::from_secs(5), ready(&mut proc, &config, "0.4.0")).await??;
    server.abort();
    Ok(())
}

#[tokio::test]
async fn trial_snapshot_restores_previous_links_state_and_cleans_trial_only_references()
-> Result<()> {
    use apsp_adapter_sdk::{Prepared, RuntimeSpec};
    let temporary =
        std::env::temp_dir().join(format!("apsp-supervisor-rollback-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&temporary)?;
    let fixture = Fixture(std::fs::canonicalize(temporary)?);
    let config = Config {
        install_root: fixture.0.join("install"),
        runtime_root: fixture.0.join("runtime"),
        state_db: fixture.0.join("state/state.db"),
        agent_root: fixture.0.join("core"),
        ..Config::default()
    };
    std::fs::create_dir_all(config.state_db.parent().unwrap())?;
    let v1_dir = config.install_root.join("demo/1.0.0");
    let v2_dir = config.install_root.join("demo/1.0.0-aaaaaaaaaaaa");
    let rev1_dir = config.runtime_root.join("demo@main/revisions/1");
    let rev2_dir = config.runtime_root.join("demo@main/revisions/2");
    for dir in [&v1_dir, &v2_dir, &rev1_dir, &rev2_dir] {
        std::fs::create_dir_all(dir)?;
    }
    SYSTEM
        .atomic_symlink(&config.install_root.join("demo/current"), &v1_dir)
        .await?;
    SYSTEM
        .atomic_symlink(&config.runtime_root.join("demo@main/current"), &rev1_dir)
        .await?;

    // Unrelated pre-existing external link must never be deleted by trial cleanup.
    let unmanaged_dir = fixture.0.join("external-target");
    std::fs::create_dir_all(&unmanaged_dir)?;
    std::fs::create_dir_all(config.install_root.join("unmanaged"))?;
    SYSTEM
        .atomic_symlink(
            &config.install_root.join("unmanaged/current"),
            &unmanaged_dir,
        )
        .await?;

    let pre_prepared = Prepared {
        spec: RuntimeSpec {
            revision: 1,
            kernel_version: "1.0.0".into(),
            config_hash: "a".repeat(64),
            binary_path: v1_dir.join("demo"),
            revision_dir: rev1_dir.clone(),
            stats_listen: "127.0.0.1:19001".into(),
            files: std::collections::BTreeMap::new(),
        },
        listen_ports: vec![],
    };
    {
        let mut db = crate::State::open(&config.state_db)?;
        db.set_json("applied:demo", &pre_prepared)?;
        db.set_json("desired:demo", &true)?;
        db.set_json("health:demo", &true)?;
    }

    let snapshot = capture_trial_snapshot(&config)?;

    // Simulate failed candidate mutations: flipped `demo` to v2/rev2 AND added a brand-new
    // bundled `extra` component + `extra@main/current` + `applied:extra` where none existed!
    SYSTEM
        .atomic_symlink(&config.install_root.join("demo/current"), &v2_dir)
        .await?;
    SYSTEM
        .atomic_symlink(&config.runtime_root.join("demo@main/current"), &rev2_dir)
        .await?;
    let extra_v1 = config.install_root.join("extra/2.0.0");
    let extra_rev1 = config.runtime_root.join("extra@main/revisions/1");
    std::fs::create_dir_all(&extra_v1)?;
    std::fs::create_dir_all(&extra_rev1)?;
    std::fs::write(extra_v1.join("extra"), b"bin")?;
    let bundled_manifest = apsp_protocol::bundled::BundledManifest {
        schema: 1,
        component_name: "extra".into(),
        binary_name: "extra".into(),
        version: "2.0.0".into(),
        bundle_sha256: "b".repeat(64),
        binary_sha256: "c".repeat(64),
        files: std::collections::BTreeMap::from([(
            "extra".into(),
            apsp_protocol::bundled::BundledFileRecord {
                sha256: "c".repeat(64),
                size: 3,
                executable: true,
            },
        )]),
    };
    std::fs::write(
        extra_v1.join(apsp_protocol::bundled::BUNDLED_MARKER),
        serde_json::to_vec(&bundled_manifest)?,
    )?;
    SYSTEM
        .atomic_symlink(&config.install_root.join("extra/current"), &extra_v1)
        .await?;
    SYSTEM
        .atomic_symlink(&config.runtime_root.join("extra@main/current"), &extra_rev1)
        .await?;
    let trial_demo = Prepared {
        spec: RuntimeSpec {
            revision: 2,
            kernel_version: "1.0.0".into(),
            config_hash: "d".repeat(64),
            binary_path: v2_dir.join("demo"),
            revision_dir: rev2_dir.clone(),
            stats_listen: "127.0.0.1:19001".into(),
            files: std::collections::BTreeMap::new(),
        },
        listen_ports: vec![],
    };
    let trial_extra = Prepared {
        spec: RuntimeSpec {
            revision: 1,
            kernel_version: "2.0.0".into(),
            config_hash: "e".repeat(64),
            binary_path: extra_v1.join("extra"),
            revision_dir: extra_rev1.clone(),
            stats_listen: "127.0.0.1:19002".into(),
            files: std::collections::BTreeMap::new(),
        },
        listen_ports: vec![],
    };
    {
        let mut db = crate::State::open(&config.state_db)?;
        db.set_json("applied:demo", &trial_demo)?;
        db.set_json("health:demo", &false)?;
        db.set_json("applied:extra", &trial_extra)?;
        db.set_json("desired:extra", &true)?;
    }

    let mut up_state = UpgradeState {
        current: "1.0.0".into(),
        trial_snapshot: Some(snapshot),
        ..UpgradeState::default()
    };
    restore_trial_state(&config, &mut up_state, &SYSTEM).await?;
    assert!(up_state.trial_snapshot.is_none());
    assert_eq!(
        std::fs::canonicalize(reference(&config.install_root.join("demo/current"))?)?,
        std::fs::canonicalize(&v1_dir)?
    );
    assert_eq!(
        std::fs::canonicalize(reference(&config.runtime_root.join("demo@main/current"))?)?,
        std::fs::canonicalize(&rev1_dir)?
    );
    assert!(!config.install_root.join("extra/current").exists());
    assert!(!config.runtime_root.join("extra@main/current").exists());
    assert_eq!(
        std::fs::canonicalize(reference(&config.install_root.join("unmanaged/current"))?)?,
        std::fs::canonicalize(&unmanaged_dir)?
    );

    let db = crate::State::open(&config.state_db)?;
    assert_eq!(
        db.get_json::<Prepared>("applied:demo")?,
        Some(pre_prepared.clone())
    );
    assert_eq!(db.get_json::<bool>("desired:demo")?, Some(true));
    assert_eq!(db.get_json::<bool>("health:demo")?, Some(true));
    assert_eq!(db.get_json::<Prepared>("applied:extra")?, None);
    assert_eq!(db.get_json::<bool>("desired:extra")?, None);
    let intents = db.pending_intents()?;
    assert_eq!(intents.len(), 2);
    Ok(())
}

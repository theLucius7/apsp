#![forbid(unsafe_code)]

use apsp_agent_core::{Config, State};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    config: Config,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "apsp-monitor-cli-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let config = Config {
            panel_url: "http://127.0.0.1:9".into(),
            identity_dir: root.join("identity"),
            state_db: root.join("state.db"),
            runtime_root: root.join("runtime"),
            install_root: root.join("artifacts"),
            agent_root: root.join("core"),
            status_socket: root.join("status.sock"),
            ..Config::default()
        };
        let path = root.join("agent.toml");
        fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
        Self { root, config, path }
    }

    fn reject_all_monitor_entrypoints(&self) {
        for command in ["run", "supervise", "install-service"] {
            let output = Command::new(env!("CARGO_BIN_EXE_apsp-agent"))
                .arg("--config")
                .arg(&self.path)
                .args([command, "--monitor-only"])
                .output()
                .unwrap();
            assert!(!output.status.success(), "{command} accepted managed state");
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("monitor-only refuses existing managed"),
                "{command} did not reject the mode change before other startup work: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(!self.config.status_socket.exists());
        assert!(!self.config.agent_root.exists());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn every_monitor_cli_entrypoint_rejects_and_preserves_managed_ledger() {
    let fixture = Fixture::new();
    let mut state = State::open(&fixture.config.state_db).unwrap();
    let snapshot = serde_json::json!({"fixture": "managed configuration must survive rejection"});
    state.set_json("applied:demo", &snapshot).unwrap();
    fixture.reject_all_monitor_entrypoints();
    assert_eq!(
        state.get_json::<serde_json::Value>("applied:demo").unwrap(),
        Some(snapshot)
    );
    assert!(!fixture.config.runtime_root.exists());
    assert!(!fixture.config.install_root.exists());
}

#[test]
fn every_monitor_cli_entrypoint_rejects_orphaned_runtime_credentials_without_a_ledger() {
    let fixture = Fixture::new();
    let revision = fixture.config.runtime_root.join("demo@main/revisions/1");
    fs::create_dir_all(&revision).unwrap();
    let configuration = revision.join("config.json");
    fs::write(&configuration, "TEST_ONLY_runtime_credentials").unwrap();
    fixture.reject_all_monitor_entrypoints();
    assert_eq!(
        fs::read_to_string(configuration).unwrap(),
        "TEST_ONLY_runtime_credentials"
    );
    assert!(!fixture.config.state_db.exists());
}

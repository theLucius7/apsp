#![forbid(unsafe_code)]
#![cfg(unix)]

use apsp_adapter_sdk::{Counter, Plan, Prepared, Privileged, RuntimeSpec};
use apsp_agent_core::{
    config::Config,
    fake::{FakeAdapter, FakeServiceManager},
    reconcile::{ApplyIntent, Reconciler},
    state::{IntentRecord, SharedState, State},
    system::SystemOps,
};
use apsp_protocol::ApplyStatus;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};
use uuid::Uuid;

struct Fixture {
    directory: PathBuf,
    config: Config,
    state: SharedState,
    adapter: Arc<FakeAdapter>,
    services: Arc<FakeServiceManager>,
    reconciler: Arc<Reconciler>,
}

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("apsp-reconcile-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let config = Config {
            settings: apsp_protocol::AgentSettings::default(),
            panel_url: "http://127.0.0.1:8080".into(),
            panel_ca_file: None,
            identity_dir: directory.join("identity"),
            state_db: directory.join("state.db"),
            runtime_root: directory.join("runtime"),
            install_root: directory.join("install"),
            agent_root: directory.join("core"),
            status_socket: directory.join("status.sock"),
            operation_timeout_secs: 1,
            public_ips: vec![],
            allow_remote_commands: false,
        };
        let state = Arc::new(Mutex::new(State::open(&config.state_db).unwrap()));
        let adapter = Arc::new(FakeAdapter::default());
        let services = Arc::new(FakeServiceManager::default());
        let reconciler = Arc::new(
            Reconciler::new(
                config.clone(),
                state.clone(),
                adapter.clone(),
                Arc::new(SystemOps),
                services.clone(),
            )
            .with_trusted_keys(release_support::trusted_keys()),
        );
        Self {
            directory,
            config,
            state,
            adapter,
            services,
            reconciler,
        }
    }
    fn target(&self, rev: u64, kernel: &str, hash: &str) -> Prepared {
        let revision_dir = self
            .config
            .runtime_root
            .join("demo@main/revisions")
            .join(rev.to_string());
        let kernel_dir = self.config.install_root.join("demo").join(kernel);
        fs::create_dir_all(&revision_dir).unwrap();
        fs::create_dir_all(&kernel_dir).unwrap();
        fs::write(revision_dir.join("config.json"), "{}").unwrap();
        fs::write(kernel_dir.join("demo"), b"fake executable").unwrap();
        release_support::install_proof(
            &kernel_dir,
            &release_support::proof_for_archive(
                "demo",
                kernel,
                "demo",
                b"cached archive",
                b"fake executable",
            ),
        );
        Prepared {
            spec: RuntimeSpec {
                revision: rev,
                kernel_version: kernel.into(),
                config_hash: hash.into(),
                binary_path: kernel_dir.join("demo"),
                revision_dir,
                stats_listen: "127.0.0.1:18085".into(),
                files: BTreeMap::from([("config.json".into(), "{}".into())]),
            },
            listen_ports: vec![],
        }
    }
    fn applied(&self) -> Option<Prepared> {
        self.state.lock().unwrap().get_json("applied:demo").unwrap()
    }
    fn set_counter(&self, up: u64, down: u64) {
        *self.adapter.counters.lock().unwrap() = vec![Counter {
            stat_name: "u1_n3".into(),
            uplink: up,
            downlink: down,
        }];
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test]
async fn changed_config_reloads_kernel_change_restarts_and_same_hash_is_noop() {
    let test = Fixture::new();
    for target in [
        test.target(1, "1", "a"),
        test.target(2, "1", "a"),
        test.target(3, "1", "b"),
        test.target(4, "2", "b"),
    ] {
        assert_eq!(
            test.reconciler.apply_prepared(target).await.unwrap().status,
            ApplyStatus::Applied
        );
    }
    assert_eq!(
        *test.services.actions.lock().unwrap(),
        ["restart:demo@main", "reload:demo@main", "restart:demo@main"]
    );
    assert_eq!(test.applied().unwrap().spec.revision, 4);
    assert!(
        test.state
            .lock()
            .unwrap()
            .pending_intents()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn failed_health_rolls_back_links_state_and_captures_terminal_usage() {
    let test = Fixture::new();
    let previous = test.target(1, "1", "a");
    test.reconciler
        .apply_prepared(previous.clone())
        .await
        .unwrap();
    test.set_counter(100, 200);
    test.reconciler.sample_usage().await.unwrap();
    test.set_counter(150, 280);
    test.adapter.fail_health.store(true, Ordering::SeqCst);
    let result = test
        .reconciler
        .apply_prepared(test.target(2, "2", "b"))
        .await
        .unwrap();
    assert_eq!(result.status, ApplyStatus::Failed);
    assert!(result.healthy);
    assert_eq!(test.applied(), Some(previous.clone()));
    assert_eq!(
        fs::read_link(test.config.runtime_root.join("demo@main/current")).unwrap(),
        previous.spec.revision_dir
    );
    assert_eq!(
        fs::read_link(test.config.install_root.join("demo/current")).unwrap(),
        previous.spec.binary_path.parent().unwrap()
    );
    test.set_counter(10, 20);
    test.reconciler.sample_usage().await.unwrap();
    let batches = test.state.lock().unwrap().pending_usage().unwrap();
    assert_eq!(
        batches
            .iter()
            .flat_map(|batch| &batch.records)
            .map(|record| record.uplink)
            .sum::<u64>(),
        160
    );
    assert_eq!(
        batches
            .iter()
            .flat_map(|batch| &batch.records)
            .map(|record| record.downlink)
            .sum::<u64>(),
        300
    );
    assert!(
        test.state
            .lock()
            .unwrap()
            .pending_intents()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn extended_health_budget_allows_startup_but_still_rolls_back_on_timeout() {
    let test = Fixture::new();
    test.adapter.health_budget_secs.store(2, Ordering::SeqCst);
    test.adapter
        .next_health_delay_ms
        .store(1200, Ordering::SeqCst);
    let first = test
        .reconciler
        .apply_prepared(test.target(1, "1", "a"))
        .await
        .unwrap();
    assert_eq!(first.status, ApplyStatus::Applied);
    test.adapter
        .next_health_delay_ms
        .store(2500, Ordering::SeqCst);
    let failed = test
        .reconciler
        .apply_prepared(test.target(2, "1", "b"))
        .await
        .unwrap();
    assert_eq!(failed.status, ApplyStatus::Failed);
    assert!(failed.healthy);
    assert!(failed.error.unwrap().contains("health check timed out"));
    assert_eq!(test.applied().unwrap().spec.revision, 1);
}

#[tokio::test]
async fn missing_terminal_counters_prevent_service_disruption() {
    let test = Fixture::new();
    test.reconciler
        .apply_prepared(test.target(1, "1", "a"))
        .await
        .unwrap();
    let actions = test.services.actions.lock().unwrap().clone();
    test.adapter.fail_counters.store(true, Ordering::SeqCst);
    let result = test
        .reconciler
        .apply_prepared(test.target(2, "1", "b"))
        .await
        .unwrap();
    assert_eq!(result.status, ApplyStatus::Failed);
    assert_eq!(*test.services.actions.lock().unwrap(), actions);
    assert_eq!(test.applied().unwrap().spec.revision, 1);
    assert!(
        test.state
            .lock()
            .unwrap()
            .pending_intents()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn unfinished_intent_is_recovered_after_database_reopen() {
    let test = Fixture::new();
    let previous = test.target(1, "1", "a");
    test.reconciler
        .apply_prepared(previous.clone())
        .await
        .unwrap();
    let target = test.target(2, "2", "b");
    let op_id = Uuid::new_v4();
    test.state
        .lock()
        .unwrap()
        .begin_intent(&IntentRecord {
            op_id,
            module: "demo".into(),
            payload: serde_json::to_value(ApplyIntent {
                previous: Some(previous.clone()),
                target: target.clone(),
                plan: Plan::Restart,
            })
            .unwrap(),
        })
        .unwrap();
    SystemOps
        .atomic_symlink(
            &test.config.runtime_root.join("demo@main/current"),
            &target.spec.revision_dir,
        )
        .await
        .unwrap();
    SystemOps
        .atomic_symlink(
            &test.config.install_root.join("demo/current"),
            target.spec.binary_path.parent().unwrap(),
        )
        .await
        .unwrap();
    let reopened = Arc::new(Mutex::new(State::open(&test.config.state_db).unwrap()));
    let restarted = Reconciler::new(
        test.config.clone(),
        reopened.clone(),
        test.adapter.clone(),
        Arc::new(SystemOps),
        test.services.clone(),
    )
    .with_trusted_keys(release_support::trusted_keys());
    restarted.recover().await.unwrap();
    assert!(
        reopened
            .lock()
            .unwrap()
            .pending_intents()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        reopened
            .lock()
            .unwrap()
            .get_json::<Prepared>("applied:demo")
            .unwrap(),
        Some(previous.clone())
    );
    assert_eq!(
        fs::read_link(test.config.runtime_root.join("demo@main/current")).unwrap(),
        previous.spec.revision_dir
    );
}

#[tokio::test]
async fn queued_versions_are_coalesced_to_the_latest_after_current_apply() {
    let test = Fixture::new();
    test.adapter.apply_delay_ms.store(100, Ordering::SeqCst);
    let first = test.target(1, "1", "a");
    let middle = test.target(2, "1", "b");
    let latest = test.target(3, "1", "c");
    let runner = test.reconciler.clone();
    let first = tokio::spawn(async move { runner.apply_prepared(first).await.unwrap() });
    tokio::time::timeout(Duration::from_secs(1), async {
        while test.adapter.applied.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let runner = test.reconciler.clone();
    let middle = tokio::spawn(async move { runner.apply_prepared(middle).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(10)).await;
    let runner = test.reconciler.clone();
    let latest = tokio::spawn(async move { runner.apply_prepared(latest).await.unwrap() });
    assert_eq!(first.await.unwrap().rev, 1);
    assert_eq!(middle.await.unwrap().rev, 3);
    assert_eq!(latest.await.unwrap().rev, 3);
    assert_eq!(*test.adapter.applied.lock().unwrap(), [1, 3]);
}

#[tokio::test]
async fn timed_out_initial_apply_stops_runtime_and_clears_intent() {
    let test = Fixture::new();
    test.adapter.apply_delay_ms.store(1500, Ordering::SeqCst);
    let result = test
        .reconciler
        .apply_prepared(test.target(1, "1", "a"))
        .await
        .unwrap();
    assert_eq!(result.status, ApplyStatus::Failed);
    assert!(!result.healthy);
    assert!(result.error.unwrap().contains("timed out"));
    assert!(test.applied().is_none());
    assert!(
        test.state
            .lock()
            .unwrap()
            .pending_intents()
            .unwrap()
            .is_empty()
    );
    assert!(!test.services.active.load(Ordering::SeqCst));
}

#[tokio::test]
async fn unsigned_or_changed_prepared_targets_do_not_start_or_create_intents() {
    let test = Fixture::new();
    let target = test.target(1, "1", "a");
    fs::remove_file(
        target
            .spec
            .binary_path
            .parent()
            .unwrap()
            .join("release.json"),
    )
    .unwrap();
    let result = test.reconciler.apply_prepared(target).await.unwrap();
    assert_eq!(result.status, ApplyStatus::Failed);
    assert!(test.services.actions.lock().unwrap().is_empty());
    assert!(
        test.state
            .lock()
            .unwrap()
            .pending_intents()
            .unwrap()
            .is_empty()
    );
    assert!(
        test.state
            .lock()
            .unwrap()
            .get_json::<serde_json::Value>("artifact_error:demo")
            .unwrap()
            .is_some()
    );
    let target = test.target(2, "2", "b");
    fs::write(&target.spec.binary_path, b"tampered executable").unwrap();
    assert_eq!(
        test.reconciler.apply_prepared(target).await.unwrap().status,
        ApplyStatus::Failed
    );
    assert!(test.services.actions.lock().unwrap().is_empty());
}

#[tokio::test]
async fn untrusted_rollback_is_preserved_as_failed_recovery_without_restart() {
    let test = Fixture::new();
    let previous = test.target(1, "1", "a");
    test.reconciler
        .apply_prepared(previous.clone())
        .await
        .unwrap();
    let target = test.target(2, "2", "b");
    let op_id = Uuid::new_v4();
    test.state
        .lock()
        .unwrap()
        .begin_intent(&IntentRecord {
            op_id,
            module: "demo".into(),
            payload: serde_json::to_value(ApplyIntent {
                previous: Some(previous.clone()),
                target,
                plan: Plan::Restart,
            })
            .unwrap(),
        })
        .unwrap();
    fs::write(&previous.spec.binary_path, b"untrusted executable").unwrap();
    let actions = test.services.actions.lock().unwrap().clone();
    assert!(
        test.reconciler
            .recover()
            .await
            .unwrap_err()
            .to_string()
            .contains("intent retained")
    );
    assert_eq!(*test.services.actions.lock().unwrap(), actions);
    assert_eq!(
        test.state.lock().unwrap().pending_intents().unwrap().len(),
        1
    );
    assert_eq!(
        test.state
            .lock()
            .unwrap()
            .get_json::<bool>("health:demo")
            .unwrap(),
        Some(false)
    );
    assert!(
        test.state
            .lock()
            .unwrap()
            .get_json::<serde_json::Value>("artifact_error:demo")
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn invalid_applied_artifact_reports_unhealthy_without_switching_to_a_new_target() {
    let test = Fixture::new();
    let previous = test.target(1, "1", "a");
    test.reconciler
        .apply_prepared(previous.clone())
        .await
        .unwrap();
    let target = test.target(2, "2", "b");
    fs::write(&previous.spec.binary_path, b"untrusted executable").unwrap();
    let actions = test.services.actions.lock().unwrap().clone();
    let result = test.reconciler.apply_prepared(target).await.unwrap();
    assert_eq!(result.status, ApplyStatus::Failed);
    assert!(!result.healthy);
    assert_eq!(
        test.state
            .lock()
            .unwrap()
            .get_json::<bool>("health:demo")
            .unwrap(),
        Some(false)
    );
    assert_eq!(*test.services.actions.lock().unwrap(), actions);
    assert_eq!(test.applied(), Some(previous));
    assert!(
        test.state
            .lock()
            .unwrap()
            .pending_intents()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn explicit_restart_checks_revision_signature_counters_and_recovers_failed_health() {
    let test = Fixture::new();
    let target = test.target(1, "1", "a");
    test.reconciler
        .apply_prepared(target.clone())
        .await
        .unwrap();
    let initial = test.services.actions.lock().unwrap().len();
    assert!(test.reconciler.restart_applied(2).await.is_err());
    assert_eq!(test.services.actions.lock().unwrap().len(), initial);
    test.adapter.fail_counters.store(true, Ordering::SeqCst);
    assert!(test.reconciler.restart_applied(1).await.is_err());
    assert_eq!(test.services.actions.lock().unwrap().len(), initial);
    test.adapter.fail_counters.store(false, Ordering::SeqCst);
    test.adapter.fail_prepare.store(true, Ordering::SeqCst);
    assert!(test.reconciler.restart_applied(1).await.is_err());
    assert_eq!(test.services.actions.lock().unwrap().len(), initial);
    test.adapter.fail_prepare.store(false, Ordering::SeqCst);
    test.adapter.fail_health.store(true, Ordering::SeqCst);
    assert!(test.reconciler.restart_applied(1).await.is_err());
    assert_eq!(test.services.actions.lock().unwrap().len(), initial + 2);
    assert_eq!(test.applied().unwrap().spec.revision, 1);
    assert!(
        test.state
            .lock()
            .unwrap()
            .pending_intents()
            .unwrap()
            .is_empty()
    );
    test.reconciler.restart_applied(1).await.unwrap();
    assert_eq!(test.services.actions.lock().unwrap().len(), initial + 3);
    fs::write(&target.spec.binary_path, "untrusted").unwrap();
    assert!(test.reconciler.restart_applied(1).await.is_err());
    assert_eq!(test.services.actions.lock().unwrap().len(), initial + 3);
}

#[tokio::test]
async fn dependency_barrier_survives_reopen_and_blocks_apply_restart_and_recovery_regression() {
    use apsp_protocol::{
        RuntimeValidationError, RuntimeValidationOperation, RuntimeValidationRequest,
    };
    let test = Fixture::new();
    let mut previous = test.target(1, "1", &"a".repeat(64));
    previous.spec.files.insert(
        "runtime-constraints.json".into(),
        serde_json::json!({"schema":1,"active":{"dependency:1":1},"retired":{}}).to_string(),
    );
    test.reconciler
        .apply_prepared(previous.clone())
        .await
        .unwrap();
    let mut current = test.target(2, "1", &"b".repeat(64));
    current.spec.files.insert(
        "runtime-constraints.json".into(),
        serde_json::json!({"schema":1,"active":{"dependency:1":2},"retired":{}}).to_string(),
    );
    test.reconciler
        .apply_prepared(current.clone())
        .await
        .unwrap();
    let mut request = RuntimeValidationRequest {
        id: Uuid::new_v4(),
        module: "demo".into(),
        scope: "dependency:1".into(),
        generation: 2,
        operation: RuntimeValidationOperation::Barrier,
        revision: 2,
        config_hash: "b".repeat(64),
        expires_at: apsp_protocol::now_timestamp() + 60,
    };
    request.config_hash = "c".repeat(64);
    assert_eq!(
        test.reconciler.validate_runtime_dependency(&request).await,
        Err(RuntimeValidationError::RevisionChanged)
    );
    request.config_hash = "b".repeat(64);
    request.expires_at = apsp_protocol::now_timestamp() - 1;
    assert_eq!(
        test.reconciler.validate_runtime_dependency(&request).await,
        Err(RuntimeValidationError::Expired)
    );
    request.expires_at = apsp_protocol::now_timestamp() + 60;
    let unfinished = Uuid::new_v4();
    test.state
        .lock()
        .unwrap()
        .begin_intent(&IntentRecord {
            op_id: unfinished,
            module: "demo".into(),
            payload: serde_json::to_value(ApplyIntent {
                previous: Some(previous.clone()),
                target: current.clone(),
                plan: Plan::Restart,
            })
            .unwrap(),
        })
        .unwrap();
    assert_eq!(
        test.reconciler.validate_runtime_dependency(&request).await,
        Err(RuntimeValidationError::ValidationFailed)
    );
    assert!(
        test.state
            .lock()
            .unwrap()
            .get_json::<serde_json::Value>("runtime_generation_floor:demo")
            .unwrap()
            .is_none()
    );
    test.state
        .lock()
        .unwrap()
        .finish_intent(unfinished)
        .unwrap();
    test.reconciler
        .validate_runtime_dependency(&request)
        .await
        .unwrap();
    test.reconciler
        .validate_runtime_dependency(&request)
        .await
        .unwrap();
    let reopened = Arc::new(Mutex::new(State::open(&test.config.state_db).unwrap()));
    let restarted = Reconciler::new(
        test.config.clone(),
        reopened.clone(),
        test.adapter.clone(),
        Arc::new(SystemOps),
        test.services.clone(),
    )
    .with_trusted_keys(release_support::trusted_keys());
    let mut unrelated = current.clone();
    unrelated.spec.revision = 3;
    unrelated.spec.config_hash = "c".repeat(64);
    assert_eq!(
        restarted.apply_prepared(unrelated).await.unwrap().status,
        ApplyStatus::Applied
    );
    assert_eq!(
        restarted.validate_runtime_dependency(&request).await,
        Err(RuntimeValidationError::RevisionChanged)
    );
    request.id = Uuid::new_v4();
    request.revision = 3;
    request.config_hash = "c".repeat(64);
    restarted
        .validate_runtime_dependency(&request)
        .await
        .unwrap();
    let actions = test.services.actions.lock().unwrap().clone();
    assert_eq!(
        restarted
            .apply_prepared(previous.clone())
            .await
            .unwrap()
            .status,
        ApplyStatus::Failed
    );
    let absent = test.target(3, "1", &"c".repeat(64));
    assert_eq!(
        restarted.apply_prepared(absent).await.unwrap().status,
        ApplyStatus::Failed
    );
    assert_eq!(*test.services.actions.lock().unwrap(), actions);
    let mut retired = test.target(4, "1", &"d".repeat(64));
    retired.spec.files.insert(
        "runtime-constraints.json".into(),
        serde_json::json!({"schema":1,"active":{},"retired":{"dependency:1":2}}).to_string(),
    );
    assert_eq!(
        restarted
            .apply_prepared(retired.clone())
            .await
            .unwrap()
            .status,
        ApplyStatus::Applied
    );
    restarted.restart_applied(4).await.unwrap();
    let op_id = Uuid::new_v4();
    reopened
        .lock()
        .unwrap()
        .begin_intent(&IntentRecord {
            op_id,
            module: "demo".into(),
            payload: serde_json::to_value(ApplyIntent {
                previous: Some(previous.clone()),
                target: retired,
                plan: Plan::Restart,
            })
            .unwrap(),
        })
        .unwrap();
    let actions = test.services.actions.lock().unwrap().clone();
    assert!(restarted.recover().await.is_err());
    assert_eq!(*test.services.actions.lock().unwrap(), actions);
    assert_eq!(reopened.lock().unwrap().pending_intents().unwrap().len(), 1);
    reopened.lock().unwrap().finish_intent(op_id).unwrap();
    reopened
        .lock()
        .unwrap()
        .set_json("applied:demo", &previous)
        .unwrap();
    assert!(restarted.restart_applied(1).await.is_err());
    assert_eq!(*test.services.actions.lock().unwrap(), actions);
}

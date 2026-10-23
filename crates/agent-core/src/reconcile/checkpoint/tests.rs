use super::*;
use crate::{
    Config, SharedState, State,
    fake::{FakeAdapter, FakeServiceManager},
    state::IntentRecord,
    system::SystemOps,
};
use apsp_adapter_sdk::{BoxFuture, Plan, RuntimeSpec, ServiceManager};
use apsp_protocol::{
    ApplyStatus, Bundle, RuntimeCheckpointRequest, RuntimePathProbeRequest, RuntimePathProbeResult,
    RuntimeRecoveryBarrierRequest, now_timestamp,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

struct ObservedServices {
    inner: FakeServiceManager,
    generation: AtomicU64,
    root: PathBuf,
    incorrect: Mutex<Option<RuntimeInstance>>,
}
impl ServiceManager for ObservedServices {
    fn supports_runtime_checkpoint(&self) -> bool {
        true
    }
    fn runtime_instance<'a>(&'a self, _unit: &'a str) -> BoxFuture<'a, RuntimeInstance> {
        Box::pin(async move {
            anyhow::ensure!(
                self.inner.active.load(Ordering::SeqCst),
                "fixture runtime is stopped"
            );
            if let Some(instance) = self.incorrect.lock().unwrap().clone() {
                return Ok(instance);
            }
            Ok(RuntimeInstance {
                instance_id: format!(
                    "fixture-generation-{}",
                    self.generation.load(Ordering::SeqCst)
                ),
                binary_path: fs::canonicalize(
                    fs::read_link(self.root.join("install/demo/current"))?.join("demo"),
                )?,
                config_path: self.root.join("runtime/demo@main/current/config.json"),
            })
        })
    }
    fn restart<'a>(&'a self, unit: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.inner.restart(unit).await?;
            self.generation.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
    fn reload<'a>(&'a self, unit: &'a str) -> BoxFuture<'a, ()> {
        self.inner.reload(unit)
    }
    fn stop<'a>(&'a self, unit: &'a str) -> BoxFuture<'a, ()> {
        self.inner.stop(unit)
    }
    fn is_active<'a>(&'a self, unit: &'a str) -> BoxFuture<'a, bool> {
        self.inner.is_active(unit)
    }
}

struct Fixture {
    root: PathBuf,
    config: Config,
    state: SharedState,
    adapter: Arc<FakeAdapter>,
    services: Arc<ObservedServices>,
    reconciler: Arc<Reconciler>,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("sn-runtime-proof-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        let root = fs::canonicalize(path).unwrap();
        let config = Config {
            settings: Default::default(),
            panel_url: "http://127.0.0.1:8080".into(),
            panel_ca_file: None,
            identity_dir: root.join("identity"),
            state_db: root.join("state.db"),
            runtime_root: root.join("runtime"),
            install_root: root.join("install"),
            agent_root: root.join("agent"),
            status_socket: root.join("status.sock"),
            operation_timeout_secs: 2,
            public_ips: vec![],
            allow_remote_commands: false,
        };
        let state = Arc::new(Mutex::new(State::open(&config.state_db).unwrap()));
        let adapter = Arc::new(FakeAdapter::default());
        let services = Arc::new(ObservedServices {
            inner: FakeServiceManager::default(),
            generation: AtomicU64::new(0),
            root: root.clone(),
            incorrect: Mutex::new(None),
        });
        let reconciler = Arc::new(
            Reconciler::new(
                config.clone(),
                state.clone(),
                adapter.clone(),
                Arc::new(SystemOps),
                services.clone(),
            )
            .with_trusted_keys(crate::release_test_support::trusted_keys()),
        );
        Self {
            root,
            config,
            state,
            adapter,
            services,
            reconciler,
        }
    }
    fn target(&self, revision: u64, contents: &str) -> Prepared {
        let directory = self
            .config
            .runtime_root
            .join(format!("demo@main/revisions/{revision}"));
        let binary = self.config.install_root.join("demo/1/demo");
        fs::create_dir_all(&directory).unwrap();
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"fixture executable").unwrap();
        fs::write(directory.join("config.json"), contents).unwrap();
        crate::release_test_support::install_proof(
            binary.parent().unwrap(),
            &crate::release_test_support::proof_for_archive(
                "demo",
                "1",
                "demo",
                b"fixture archive",
                b"fixture executable",
            ),
        );
        let files = BTreeMap::from([("config.json".into(), contents.into())]);
        let hash = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&Bundle {
                    files: files.clone()
                })
                .unwrap()
            )
        );
        Prepared {
            spec: RuntimeSpec {
                revision,
                kernel_version: "1".into(),
                config_hash: hash,
                binary_path: binary,
                revision_dir: directory,
                stats_listen: "127.0.0.1:18085".into(),
                files,
            },
            listen_ports: vec![],
        }
    }
    async fn apply(&self, target: Prepared) {
        assert_eq!(
            self.reconciler.apply_prepared(target).await.unwrap().status,
            ApplyStatus::Applied
        );
    }
    async fn checkpoint(&self, binding: RuntimeBinding) -> RuntimeCheckpointResult {
        let request = ControlRequest::Checkpoint(RuntimeCheckpointRequest {
            request_id: Uuid::new_v4(),
            expected: binding,
            expires_at: now_timestamp() + 60,
        });
        self.state
            .lock()
            .unwrap()
            .enqueue_runtime_control(&request)
            .unwrap();
        let ControlResult::Checkpoint(result) =
            self.reconciler.runtime_control(&request).await.unwrap()
        else {
            unreachable!()
        };
        result
    }
    async fn barrier(
        &self,
        expected: RuntimeCheckpoint,
        minimum_revision: u64,
    ) -> RuntimeRecoveryBarrierResult {
        let request = ControlRequest::Barrier(RuntimeRecoveryBarrierRequest {
            request_id: Uuid::new_v4(),
            expected,
            minimum_revision,
            expires_at: now_timestamp() + 60,
        });
        self.state
            .lock()
            .unwrap()
            .enqueue_runtime_control(&request)
            .unwrap();
        let ControlResult::Barrier(result) =
            self.reconciler.runtime_control(&request).await.unwrap()
        else {
            unreachable!()
        };
        result
    }
    fn binding(target: &Prepared) -> RuntimeBinding {
        RuntimeBinding::new(
            Uuid::new_v4(),
            "demo".into(),
            target.spec.revision,
            target.spec.config_hash.clone(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn probe_request(
    fixture: &Fixture,
    expected: RuntimeCheckpoint,
) -> (ControlRequest, RuntimePathProbeResult) {
    let request = ControlRequest::Probe(RuntimePathProbeRequest {
        request_id: Uuid::new_v4(),
        expected,
        probe_id: Uuid::new_v4(),
        expires_at: now_timestamp() + 60,
    });
    fixture
        .state
        .lock()
        .unwrap()
        .enqueue_runtime_control(&request)
        .unwrap();
    let ControlResult::Probe(result) = fixture.reconciler.runtime_control(&request).await.unwrap()
    else {
        unreachable!()
    };
    (request, result)
}

#[tokio::test]
async fn path_probe_uses_exact_activation_without_apply_or_recovery_side_effects_and_replays_once()
{
    let fixture = Fixture::new();
    fixture
        .adapter
        .probe_supported
        .store(true, Ordering::SeqCst);
    let target = fixture.target(1, "{}");
    fixture.apply(target.clone()).await;
    let expected = fixture
        .checkpoint(Fixture::binding(&target))
        .await
        .observed
        .unwrap();
    let actions = fixture.services.inner.actions.lock().unwrap().clone();
    let (request, result) = probe_request(&fixture, expected.clone()).await;
    assert!(result.success);
    assert_eq!(result.observed, Some(expected));
    assert_eq!(result.elapsed_ms, Some(17));
    assert_eq!(
        fixture.services.inner.actions.lock().unwrap().clone(),
        actions
    );
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .runtime_revision_floor("demo")
            .unwrap(),
        0
    );
    assert_eq!(fixture.adapter.probe_calls.lock().unwrap().len(), 1);
    assert_eq!(
        fixture.reconciler.runtime_control(&request).await.unwrap(),
        ControlResult::Probe(result)
    );
    assert_eq!(fixture.adapter.probe_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unsupported_stale_or_unfinished_probe_never_calls_the_adapter() {
    let fixture = Fixture::new();
    let target = fixture.target(1, "{}");
    fixture.apply(target.clone()).await;
    let expected = fixture
        .checkpoint(Fixture::binding(&target))
        .await
        .observed
        .unwrap();
    assert!(!probe_request(&fixture, expected.clone()).await.1.success);
    fixture
        .adapter
        .probe_supported
        .store(true, Ordering::SeqCst);
    let mut stale = expected.clone();
    stale.activation_id = Uuid::new_v4();
    assert!(!probe_request(&fixture, stale).await.1.success);
    let intent = IntentRecord {
        op_id: Uuid::new_v4(),
        module: "demo".into(),
        payload: serde_json::json!({}),
    };
    fixture.state.lock().unwrap().begin_intent(&intent).unwrap();
    assert!(!probe_request(&fixture, expected).await.1.success);
    assert!(fixture.adapter.probe_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn activation_change_during_probe_and_private_native_failure_produce_only_redacted_receipts()
{
    let fixture = Fixture::new();
    fixture
        .adapter
        .probe_supported
        .store(true, Ordering::SeqCst);
    fixture.adapter.probe_delay_ms.store(100, Ordering::SeqCst);
    let target = fixture.target(1, "{}");
    fixture.apply(target.clone()).await;
    let binding = Fixture::binding(&target);
    let expected = fixture.checkpoint(binding.clone()).await.observed.unwrap();
    let change = async {
        fixture.adapter.probe_started.notified().await;
        fixture.services.generation.fetch_add(1, Ordering::SeqCst);
    };
    let ((_, result), ()) = tokio::join!(probe_request(&fixture, expected), change);
    assert!(!result.success && result.elapsed_ms.is_none());
    fixture.services.generation.fetch_sub(1, Ordering::SeqCst);
    let expected = fixture.checkpoint(binding).await.observed.unwrap();
    fixture.adapter.fail_probe.store(true, Ordering::SeqCst);
    let (_, result) = probe_request(&fixture, expected).await;
    assert!(!result.success);
    assert!(!result.error.unwrap().contains("private fixture"));
}

#[tokio::test]
async fn expired_or_excessive_probe_lease_never_starts_and_slow_adapter_is_cancelled() {
    let fixture = Fixture::new();
    fixture
        .adapter
        .probe_supported
        .store(true, Ordering::SeqCst);
    let target = fixture.target(1, "{}");
    fixture.apply(target.clone()).await;
    let expected = fixture
        .checkpoint(Fixture::binding(&target))
        .await
        .observed
        .unwrap();
    for expires_at in [now_timestamp() - 1, now_timestamp() + 600] {
        let request = ControlRequest::Probe(RuntimePathProbeRequest {
            request_id: Uuid::new_v4(),
            expected: expected.clone(),
            probe_id: Uuid::new_v4(),
            expires_at,
        });
        fixture
            .state
            .lock()
            .unwrap()
            .enqueue_runtime_control(&request)
            .unwrap();
        let ControlResult::Probe(result) =
            fixture.reconciler.runtime_control(&request).await.unwrap()
        else {
            unreachable!()
        };
        assert!(!result.success);
    }
    assert!(fixture.adapter.probe_calls.lock().unwrap().is_empty());
    fixture
        .adapter
        .probe_delay_ms
        .store(10000, Ordering::SeqCst);
    let start = std::time::Instant::now();
    let (_, result) = probe_request(&fixture, expected).await;
    assert!(!result.success && result.elapsed_ms.is_none());
    assert!(start.elapsed() < std::time::Duration::from_secs(7));
    assert_eq!(fixture.adapter.probe_calls.lock().unwrap().len(), 1);
    assert!(fixture.services.inner.active.load(Ordering::SeqCst));
}

#[tokio::test]
async fn exact_checkpoint_rejects_changed_files_links_binary_and_controlled_instance() {
    let fixture = Fixture::new();
    let target = fixture.target(1, "{}");
    fixture.apply(target.clone()).await;
    let binding = Fixture::binding(&target);
    assert!(fixture.checkpoint(binding.clone()).await.success);
    let actions = fixture.services.inner.actions.lock().unwrap().clone();
    fs::write(target.spec.revision_dir.join("config.json"), "[]").unwrap();
    assert!(!fixture.checkpoint(binding.clone()).await.success);
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .get_json::<bool>("health:demo")
            .unwrap(),
        Some(false)
    );
    fs::write(target.spec.revision_dir.join("config.json"), "{}").unwrap();
    fs::write(target.spec.revision_dir.join("extra.json"), "{}").unwrap();
    assert!(!fixture.checkpoint(binding.clone()).await.success);
    fs::remove_file(target.spec.revision_dir.join("extra.json")).unwrap();
    fixture.services.generation.fetch_add(1, Ordering::SeqCst);
    assert!(!fixture.checkpoint(binding.clone()).await.success);
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .get_json::<bool>("health:demo")
            .unwrap(),
        Some(false)
    );
    fixture.services.generation.fetch_sub(1, Ordering::SeqCst);
    let current = fixture.config.runtime_root.join("demo@main/current");
    fs::remove_file(&current).unwrap();
    std::os::unix::fs::symlink(&fixture.root, &current).unwrap();
    assert!(!fixture.checkpoint(binding.clone()).await.success);
    fs::remove_file(&current).unwrap();
    std::os::unix::fs::symlink(&target.spec.revision_dir, &current).unwrap();
    fs::write(&target.spec.binary_path, b"tampered executable").unwrap();
    assert!(!fixture.checkpoint(binding).await.success);
    assert_eq!(*fixture.services.inner.actions.lock().unwrap(), actions);
}

#[tokio::test]
async fn wrong_config_path_and_future_floor_are_rejected_without_a_recovery_promise() {
    let fixture = Fixture::new();
    let target = fixture.target(1, "{}");
    fixture.apply(target.clone()).await;
    let binding = Fixture::binding(&target);
    let observed = fixture.checkpoint(binding.clone()).await.observed.unwrap();
    let mut instance = fixture
        .services
        .runtime_instance("demo@main")
        .await
        .unwrap();
    instance.config_path = fixture.root.join("unmanaged.json");
    fs::write(&instance.config_path, "{}").unwrap();
    *fixture.services.incorrect.lock().unwrap() = Some(instance);
    assert!(!fixture.checkpoint(binding).await.success);
    *fixture.services.incorrect.lock().unwrap() = None;
    let request = ControlRequest::Barrier(RuntimeRecoveryBarrierRequest {
        request_id: Uuid::new_v4(),
        expected: observed,
        minimum_revision: 2,
        expires_at: now_timestamp() + 60,
    });
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .enqueue_runtime_control(&request)
            .is_err()
    );
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .runtime_revision_floor("demo")
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn expired_barrier_recovers_a_durable_result_but_never_executes_an_unstarted_floor_change() {
    let fixture = Fixture::new();
    let target = fixture.target(1, "{}");
    fixture.apply(target.clone()).await;
    let expected = fixture
        .checkpoint(Fixture::binding(&target))
        .await
        .observed
        .unwrap();
    let actions = fixture.services.inner.actions.lock().unwrap().clone();
    let expired = ControlRequest::Barrier(RuntimeRecoveryBarrierRequest {
        request_id: Uuid::new_v4(),
        expected: expected.clone(),
        minimum_revision: 1,
        expires_at: now_timestamp() - 1,
    });
    fixture
        .state
        .lock()
        .unwrap()
        .enqueue_runtime_control(&expired)
        .unwrap();
    let unstarted = fixture.reconciler.runtime_control(&expired).await.unwrap();
    let ControlResult::Barrier(failure) = &unstarted else {
        unreachable!()
    };
    assert!(!failure.success && !failure.pending_intents_clear);
    assert!(failure.observed.is_none() && failure.minimum_revision.is_none());
    assert_eq!(failure.request_id, expired.id());
    assert_eq!(failure.request_digest, expired.digest().unwrap());
    assert!(failure.error.as_deref().unwrap().contains("expired"));
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .runtime_revision_floor("demo")
            .unwrap(),
        0
    );
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .runtime_control_result(&expired)
            .unwrap(),
        Some(unstarted.clone())
    );
    let committed = ControlRequest::Barrier(RuntimeRecoveryBarrierRequest {
        request_id: Uuid::new_v4(),
        expected,
        minimum_revision: 1,
        expires_at: now_timestamp() + 60,
    });
    fixture
        .state
        .lock()
        .unwrap()
        .enqueue_runtime_control(&committed)
        .unwrap();
    let original = fixture
        .reconciler
        .runtime_control(&committed)
        .await
        .unwrap();
    let ControlResult::Barrier(success) = &original else {
        unreachable!()
    };
    assert!(success.success && success.pending_intents_clear);
    assert_eq!(success.minimum_revision, Some(1));
    fixture
        .state
        .lock()
        .unwrap()
        .set_json("clock_offset_ms", &180000i64)
        .unwrap();
    fixture.services.generation.fetch_add(1, Ordering::SeqCst);
    // Reopening the actual persistent journal models an Agent restart. Recovery
    // returns the old receipt, even after expiry and a changed live instance.
    let reopened = Arc::new(Mutex::new(State::open(&fixture.config.state_db).unwrap()));
    let reconciler = Reconciler::new(
        fixture.config.clone(),
        reopened.clone(),
        fixture.adapter.clone(),
        Arc::new(SystemOps),
        fixture.services.clone(),
    )
    .with_trusted_keys(crate::release_test_support::trusted_keys());
    assert!(!committed.valid_at(reconciler.runtime_control_now().unwrap()));
    assert_eq!(
        reopened
            .lock()
            .unwrap()
            .enqueue_runtime_control(&committed)
            .unwrap(),
        Some(original.clone())
    );
    assert_eq!(
        reconciler.runtime_control(&committed).await.unwrap(),
        original
    );
    assert_eq!(
        reconciler.runtime_control(&expired).await.unwrap(),
        unstarted
    );
    assert_eq!(
        reopened
            .lock()
            .unwrap()
            .runtime_revision_floor("demo")
            .unwrap(),
        1
    );
    assert_eq!(*fixture.services.inner.actions.lock().unwrap(), actions);
}

#[tokio::test]
async fn stable_activation_survives_noop_revision_and_result_reconstruction() {
    let fixture = Fixture::new();
    let first = fixture.target(1, "{}");
    fixture.apply(first.clone()).await;
    let before = fixture
        .checkpoint(Fixture::binding(&first))
        .await
        .observed
        .unwrap();
    let receipt = fixture.reconciler.current_result().await.unwrap();
    assert_eq!(
        fixture.reconciler.current_result().await.unwrap().op_id,
        receipt.op_id
    );
    let second = fixture.target(2, "{}");
    fixture.apply(second.clone()).await;
    let after = fixture
        .checkpoint(Fixture::binding(&second))
        .await
        .observed
        .unwrap();
    assert_eq!(before.activation_id, after.activation_id);
    assert_eq!(before.instance_id, after.instance_id);
    let new_receipt = fixture.reconciler.current_result().await.unwrap();
    assert_ne!(receipt.op_id, new_receipt.op_id);
    assert_eq!(
        fixture.reconciler.current_result().await.unwrap().op_id,
        new_receipt.op_id
    );
    assert_eq!(
        *fixture.services.inner.actions.lock().unwrap(),
        ["restart:demo@main"]
    );
}

#[tokio::test]
async fn legacy_database_checkpoint_refuses_adoption_until_explicit_new_deployment() {
    let fixture = Fixture::new();
    let first = fixture.target(1, "{}");
    fixture.apply(first.clone()).await;
    fixture
        .state
        .lock()
        .unwrap()
        .remove_json("runtime_activation:demo")
        .unwrap();
    let actions = fixture.services.inner.actions.lock().unwrap().clone();
    let result = fixture.checkpoint(Fixture::binding(&first)).await;
    assert!(!result.success);
    assert!(
        result
            .error
            .unwrap()
            .contains("administrator must create a new deployment")
    );
    assert_eq!(*fixture.services.inner.actions.lock().unwrap(), actions);
    fixture.apply(fixture.target(2, "{}")).await;
    let current = fixture.target(2, "{}");
    assert!(fixture.checkpoint(Fixture::binding(&current)).await.success);
    assert_eq!(
        fixture.services.inner.actions.lock().unwrap().len(),
        actions.len() + 1
    );
}

#[tokio::test]
async fn stale_activation_requires_a_higher_deployment_to_restart_and_recertify() {
    let fixture = Fixture::new();
    let first = fixture.target(1, "{}");
    fixture.apply(first.clone()).await;
    let binding = Fixture::binding(&first);
    let original = fixture.checkpoint(binding.clone()).await.observed.unwrap();
    let actions = fixture.services.inner.actions.lock().unwrap().clone();

    // Model a service-manager restart outside managed application: the bytes and
    // command paths remain identical, but this process never received an activation.
    fixture.services.generation.fetch_add(1, Ordering::SeqCst);
    let restarted_instance = fixture
        .services
        .runtime_instance("demo@main")
        .await
        .unwrap();
    assert_ne!(restarted_instance.instance_id, original.instance_id);
    assert!(!fixture.checkpoint(binding).await.success);
    let same_revision = fixture.reconciler.apply_prepared(first).await.unwrap();
    assert_eq!(same_revision.status, ApplyStatus::Failed);
    assert!(!same_revision.healthy);
    assert_eq!(*fixture.services.inner.actions.lock().unwrap(), actions);
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .pending_intents()
            .unwrap()
            .is_empty()
    );

    let higher = fixture.target(2, "{}");
    fixture.apply(higher.clone()).await;
    let certified = fixture
        .checkpoint(Fixture::binding(&higher))
        .await
        .observed
        .unwrap();
    assert_ne!(certified.activation_id, original.activation_id);
    assert_ne!(certified.instance_id, original.instance_id);
    assert_ne!(certified.instance_id, restarted_instance.instance_id);
    assert_eq!(certified.binding.revision, 2);
    assert_eq!(
        certified.binding.bundle_sha256,
        original.binding.bundle_sha256
    );
    assert!(certified.healthy);
    assert_eq!(
        *fixture.services.inner.actions.lock().unwrap(),
        ["restart:demo@main", "restart:demo@main"]
    );
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .pending_intents()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .get_json::<bool>("health:demo")
            .unwrap(),
        Some(true)
    );
}

#[tokio::test]
async fn wrong_barrier_instance_or_binding_never_advances_floor_and_success_prevents_rollback() {
    let fixture = Fixture::new();
    let first = fixture.target(1, "{}");
    fixture.apply(first.clone()).await;
    let second = fixture.target(2, "{\"next\":true}");
    fixture.apply(second.clone()).await;
    let observed = fixture
        .checkpoint(Fixture::binding(&second))
        .await
        .observed
        .unwrap();
    let mut wrong = observed.clone();
    wrong.instance_id = "another-instance".into();
    assert!(!fixture.barrier(wrong, 2).await.success);
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .runtime_revision_floor("demo")
            .unwrap(),
        0
    );
    let mut wrong = observed.clone();
    wrong.binding = RuntimeBinding::new(Uuid::new_v4(), "demo".into(), 2, "b".repeat(64));
    assert!(!fixture.barrier(wrong, 2).await.success);
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .runtime_revision_floor("demo")
            .unwrap(),
        0
    );
    assert!(fixture.barrier(observed, 2).await.success);
    let actions = fixture.services.inner.actions.lock().unwrap().clone();
    assert_eq!(
        fixture
            .reconciler
            .apply_prepared(first.clone())
            .await
            .unwrap()
            .status,
        ApplyStatus::Failed
    );
    let target = fixture.target(3, "{\"future\":true}");
    fixture
        .state
        .lock()
        .unwrap()
        .begin_intent(&IntentRecord {
            op_id: Uuid::new_v4(),
            module: "demo".into(),
            payload: serde_json::to_value(super::super::ApplyIntent {
                previous: Some(first),
                target,
                plan: Plan::Restart,
            })
            .unwrap(),
        })
        .unwrap();
    let error = fixture.reconciler.recover().await.unwrap_err();
    assert!(
        error
            .downcast_ref::<super::super::RecoveryBlocked>()
            .is_some()
    );
    assert_eq!(*fixture.services.inner.actions.lock().unwrap(), actions);
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .pending_intents()
            .unwrap()
            .len(),
        1
    );
    let failed = fixture
        .reconciler
        .recovery_failure_result()
        .unwrap()
        .unwrap();
    assert!(!failed.healthy);
    assert!(failed.error.as_ref().unwrap().contains("revision floor"));
    assert_eq!(
        fixture
            .reconciler
            .recovery_failure_result()
            .unwrap()
            .unwrap()
            .op_id,
        failed.op_id
    );
    let reopened = State::open(&fixture.config.state_db).unwrap();
    assert_eq!(reopened.runtime_revision_floor("demo").unwrap(), 2);
    assert_eq!(reopened.pending_runtime_results().unwrap().len(), 4);
}

#[tokio::test]
async fn health_failure_and_recovery_generate_distinct_immutable_facts() {
    let fixture = Fixture::new();
    let target = fixture.target(1, "{}");
    fixture.apply(target).await;
    let before = fixture.reconciler.current_result().await.unwrap();
    fixture
        .state
        .lock()
        .unwrap()
        .set_json("health:demo", &false)
        .unwrap();
    let failed = fixture.reconciler.current_result().await.unwrap();
    assert_ne!(before.op_id, failed.op_id);
    assert_eq!(failed.status, ApplyStatus::Failed);
    assert_eq!(
        fixture.reconciler.current_result().await.unwrap().op_id,
        failed.op_id
    );
    fixture
        .state
        .lock()
        .unwrap()
        .set_json("health:demo", &true)
        .unwrap();
    let recovered = fixture.reconciler.current_result().await.unwrap();
    assert_eq!(recovered.status, ApplyStatus::Applied);
    assert_ne!(failed.op_id, recovered.op_id);
    assert_eq!(
        fixture.reconciler.current_result().await.unwrap().op_id,
        recovered.op_id
    );
}

#[tokio::test]
async fn failed_apply_persists_its_receipt_without_retaining_the_state_mutex() {
    let fixture = Fixture::new();
    let target = fixture.target(1, "{}");
    fs::write(&target.spec.binary_path, b"tampered executable").unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fixture.reconciler.apply_prepared(target),
    )
    .await
    .expect("failed apply retained the state mutex")
    .unwrap();
    assert_eq!(result.status, ApplyStatus::Failed);
    assert!(fixture.services.inner.actions.lock().unwrap().is_empty());
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .get_json::<ApplyResult>("apply_receipt:demo")
            .unwrap()
            .unwrap(),
        result
    );
}

#[tokio::test]
async fn pending_recovery_before_barrier_changes_instance_and_requires_a_new_exact_checkpoint() {
    let fixture = Fixture::new();
    let first = fixture.target(1, "{}");
    fixture.apply(first.clone()).await;
    let before = fixture
        .checkpoint(Fixture::binding(&first))
        .await
        .observed
        .unwrap();
    let target = fixture.target(2, "{\"next\":true}");
    fixture
        .state
        .lock()
        .unwrap()
        .begin_intent(&IntentRecord {
            op_id: Uuid::new_v4(),
            module: "demo".into(),
            payload: serde_json::to_value(super::super::ApplyIntent {
                previous: Some(first),
                target,
                plan: Plan::Restart,
            })
            .unwrap(),
        })
        .unwrap();
    let actions = fixture.services.inner.actions.lock().unwrap().clone();
    assert!(!fixture.barrier(before.clone(), 1).await.success);
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .runtime_revision_floor("demo")
            .unwrap(),
        0
    );
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .pending_intents()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(*fixture.services.inner.actions.lock().unwrap(), actions);
    fixture.reconciler.recover().await.unwrap();
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .pending_intents()
            .unwrap()
            .is_empty()
    );
    let after = fixture.checkpoint(before.binding).await.observed.unwrap();
    assert_ne!(after.activation_id, before.activation_id);
    assert_ne!(after.instance_id, before.instance_id);
    assert!(fixture.barrier(after, 1).await.success);
}

#[tokio::test]
async fn read_only_checkpoint_with_an_unfinished_apply_does_not_restart_or_finish_it() {
    let fixture = Fixture::new();
    let first = fixture.target(1, "{}");
    fixture.apply(first.clone()).await;
    let binding = Fixture::binding(&first);
    let target = fixture.target(2, "{\"next\":true}");
    let intent = IntentRecord {
        op_id: Uuid::new_v4(),
        module: "demo".into(),
        payload: serde_json::to_value(super::super::ApplyIntent {
            previous: Some(first),
            target,
            plan: Plan::Restart,
        })
        .unwrap(),
    };
    fixture.state.lock().unwrap().begin_intent(&intent).unwrap();
    let actions = fixture.services.inner.actions.lock().unwrap().clone();
    let result = fixture.checkpoint(binding).await;
    assert!(!result.success);
    assert!(result.error.unwrap().contains("managed reconciliation"));
    assert_eq!(
        fixture.state.lock().unwrap().pending_intents().unwrap(),
        vec![intent]
    );
    assert_eq!(*fixture.services.inner.actions.lock().unwrap(), actions);
}

#[tokio::test]
async fn lost_ack_replays_identical_checkpoint_after_reopen_without_reinspection() {
    let fixture = Fixture::new();
    let target = fixture.target(1, "{}");
    fixture.apply(target.clone()).await;
    let request = ControlRequest::Checkpoint(RuntimeCheckpointRequest {
        request_id: Uuid::new_v4(),
        expected: Fixture::binding(&target),
        expires_at: now_timestamp() + 60,
    });
    fixture
        .state
        .lock()
        .unwrap()
        .enqueue_runtime_control(&request)
        .unwrap();
    let original = fixture.reconciler.runtime_control(&request).await.unwrap();
    fixture.services.generation.fetch_add(1, Ordering::SeqCst);
    let reopened = Arc::new(Mutex::new(State::open(&fixture.config.state_db).unwrap()));
    let reconciler = Reconciler::new(
        fixture.config.clone(),
        reopened.clone(),
        fixture.adapter.clone(),
        Arc::new(SystemOps),
        fixture.services.clone(),
    )
    .with_trusted_keys(crate::release_test_support::trusted_keys());
    assert_eq!(
        reconciler.runtime_control(&request).await.unwrap(),
        original
    );
    assert_eq!(
        reopened.lock().unwrap().pending_runtime_results().unwrap(),
        vec![original]
    );
}

#[tokio::test]
async fn explicit_runtime_restart_rotates_activation_and_obeys_the_recovery_revision_floor() {
    let fixture = Fixture::new();
    let first = fixture.target(1, "{}");
    fixture.apply(first.clone()).await;
    let binding = Fixture::binding(&first);
    let original = fixture.checkpoint(binding.clone()).await.observed.unwrap();
    assert!(fixture.barrier(original.clone(), 1).await.success);

    fixture.reconciler.restart_applied(1).await.unwrap();
    let restarted = fixture.checkpoint(binding).await.observed.unwrap();
    assert_ne!(restarted.activation_id, original.activation_id);
    assert_ne!(restarted.instance_id, original.instance_id);
    assert_eq!(restarted.binding, original.binding);
    assert!(!fixture.barrier(original, 1).await.success);
    assert!(fixture.barrier(restarted, 1).await.success);

    let second = fixture.target(2, "{\"next\":true}");
    fixture.apply(second.clone()).await;
    let observed = fixture
        .checkpoint(Fixture::binding(&second))
        .await
        .observed
        .unwrap();
    assert!(fixture.barrier(observed, 2).await.success);
    let actions = fixture.services.inner.actions.lock().unwrap().clone();
    // Model a restored older applied ledger beside the durable newer floor.
    fixture
        .state
        .lock()
        .unwrap()
        .set_json("applied:demo", &first)
        .unwrap();
    assert!(fixture.reconciler.restart_applied(1).await.is_err());
    assert_eq!(
        fixture.reconciler.runtime_snapshot().await.unwrap().healthy,
        Some(false)
    );
    assert_eq!(*fixture.services.inner.actions.lock().unwrap(), actions);
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .pending_intents()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .runtime_revision_floor("demo")
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn runtime_inspection_reports_an_externally_restarted_certified_instance_as_unhealthy() {
    let fixture = Fixture::new();
    fixture.apply(fixture.target(1, "{}")).await;
    assert_eq!(
        fixture.reconciler.runtime_snapshot().await.unwrap().healthy,
        Some(true)
    );
    let actions = fixture.services.inner.actions.lock().unwrap().clone();
    fixture.services.generation.fetch_add(1, Ordering::SeqCst);
    assert_eq!(
        fixture.reconciler.runtime_snapshot().await.unwrap().healthy,
        Some(false)
    );
    assert_eq!(*fixture.services.inner.actions.lock().unwrap(), actions);
}

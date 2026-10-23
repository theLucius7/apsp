use super::*;
use apsp_protocol::{RuntimeBinding, RuntimeCheckpoint};

fn request() -> ControlRequest {
    ControlRequest::Checkpoint(RuntimeCheckpointRequest {
        request_id: Uuid::new_v4(),
        expected: RuntimeBinding::new(Uuid::new_v4(), "demo".into(), 2, "a".repeat(64)),
        expires_at: apsp_protocol::now_timestamp() + 60,
    })
}

#[test]
fn probe_receipt_is_bound_durable_immutable_and_cannot_change_recovery_floor() {
    let directory = std::env::temp_dir().join(format!("sn-probe-ledger-{}", Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("state.db");
    let request = ControlRequest::Probe(RuntimePathProbeRequest {
        request_id: Uuid::new_v4(),
        probe_id: Uuid::new_v4(),
        expected: RuntimeCheckpoint {
            binding: RuntimeBinding::new(Uuid::new_v4(), "demo".into(), 2, "a".repeat(64)),
            activation_id: Uuid::new_v4(),
            instance_id: "fixture-instance".into(),
            healthy: true,
        },
        expires_at: apsp_protocol::now_timestamp() + 60,
    });
    let ControlRequest::Probe(value) = &request else {
        unreachable!()
    };
    let result = ControlResult::Probe(RuntimePathProbeResult {
        request_id: request.id(),
        request_digest: request.digest().unwrap(),
        observed: Some(value.expected.clone()),
        probe_id: value.probe_id,
        elapsed_ms: Some(17),
        success: true,
        error: None,
    });
    {
        let mut state = State::open(&path).unwrap();
        state.enqueue_runtime_control(&request).unwrap();
        let ControlResult::Probe(mut wrong) = result.clone() else {
            unreachable!()
        };
        wrong.probe_id = Uuid::new_v4();
        assert!(
            state
                .finish_runtime_control(&ControlResult::Probe(wrong), None)
                .is_err()
        );
        assert!(
            state
                .finish_runtime_control(&result, Some(("demo", 2)))
                .is_err()
        );
        assert_eq!(state.runtime_revision_floor("demo").unwrap(), 0);
        state.finish_runtime_control(&result, None).unwrap();
    }
    let mut state = State::open(&path).unwrap();
    assert_eq!(
        state.enqueue_runtime_control(&request).unwrap(),
        Some(result.clone())
    );
    assert_eq!(
        state.pending_runtime_results().unwrap(),
        vec![result.clone()]
    );
    let ack = RuntimeControlAck {
        request_id: request.id(),
        request_digest: request.digest().unwrap(),
    };
    assert!(
        state
            .acknowledge_runtime_control("checkpoint", &ack)
            .is_err()
    );
    state.acknowledge_runtime_control("probe", &ack).unwrap();
    assert!(state.pending_runtime_results().unwrap().is_empty());
    assert_eq!(
        state.enqueue_runtime_control(&request).unwrap(),
        Some(result)
    );
    let mut changed = value.clone();
    changed.probe_id = Uuid::new_v4();
    assert!(
        state
            .enqueue_runtime_control(&ControlRequest::Probe(changed))
            .is_err()
    );
    drop(state);
    std::fs::remove_dir_all(directory).unwrap();
}

fn failure(request: &ControlRequest) -> ControlResult {
    ControlResult::Checkpoint(RuntimeCheckpointResult {
        request_id: request.id(),
        request_digest: request.digest().unwrap(),
        observed: None,
        success: false,
        error: Some("fixture failure".into()),
    })
}

#[test]
fn different_work_cannot_reuse_an_identifier_even_after_acknowledgement() {
    let mut state = State::open(std::path::Path::new(":memory:")).unwrap();
    let original = request();
    state.enqueue_runtime_control(&original).unwrap();
    let result = failure(&original);
    state.finish_runtime_control(&result, None).unwrap();
    let ack = RuntimeControlAck {
        request_id: original.id(),
        request_digest: original.digest().unwrap(),
    };
    assert!(state.acknowledge_runtime_control("barrier", &ack).is_err());
    assert!(
        state
            .acknowledge_runtime_control(
                "checkpoint",
                &RuntimeControlAck {
                    request_digest: "b".repeat(64),
                    ..ack.clone()
                }
            )
            .is_err()
    );
    assert_eq!(
        state.pending_runtime_results().unwrap(),
        vec![result.clone()]
    );
    state
        .acknowledge_runtime_control("checkpoint", &ack)
        .unwrap();
    state
        .acknowledge_runtime_control("checkpoint", &ack)
        .unwrap();
    assert!(state.pending_runtime_results().unwrap().is_empty());
    assert_eq!(
        state.enqueue_runtime_control(&original).unwrap(),
        Some(result)
    );
    let ControlRequest::Checkpoint(mut changed) = original else {
        unreachable!()
    };
    changed.expires_at += 1;
    assert!(
        state
            .enqueue_runtime_control(&ControlRequest::Checkpoint(changed))
            .is_err()
    );
}

#[test]
fn unacknowledged_result_survives_reopening_without_expiry_eviction() {
    let directory = std::env::temp_dir().join(format!("sn-control-ledger-{}", Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("state.db");
    let mut original = request();
    if let ControlRequest::Checkpoint(value) = &mut original {
        value.expires_at = 1;
    }
    let result = failure(&original);
    {
        let mut state = State::open(&path).unwrap();
        state.enqueue_runtime_control(&original).unwrap();
        state.finish_runtime_control(&result, None).unwrap();
    }
    let mut reopened = State::open(&path).unwrap();
    assert_eq!(
        reopened.pending_runtime_results().unwrap(),
        vec![result.clone()]
    );
    assert_eq!(
        reopened.enqueue_runtime_control(&original).unwrap(),
        Some(result)
    );
    drop(reopened);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn journal_refuses_new_work_when_full_instead_of_discarding_unacknowledged_requests() {
    let mut state = State::open(std::path::Path::new(":memory:")).unwrap();
    for _ in 0..MAX_PENDING {
        state.enqueue_runtime_control(&request()).unwrap();
    }
    assert!(state.enqueue_runtime_control(&request()).is_err());
    assert_eq!(state.pending_runtime_controls().unwrap().len(), 16);
    let total: u64 = state
        .connection
        .query_row("SELECT COUNT(*) FROM runtime_control", [], |row| row.get(0))
        .unwrap();
    assert_eq!(total, MAX_PENDING);
}

#[test]
fn barrier_floor_and_result_commit_together_and_pending_intent_preserves_both() {
    let mut state = State::open(std::path::Path::new(":memory:")).unwrap();
    let expected = RuntimeCheckpoint {
        binding: RuntimeBinding::new(Uuid::new_v4(), "demo".into(), 2, "a".repeat(64)),
        activation_id: Uuid::new_v4(),
        instance_id: "fixture-instance".into(),
        healthy: true,
    };
    let request = ControlRequest::Barrier(RuntimeRecoveryBarrierRequest {
        request_id: Uuid::new_v4(),
        expected: expected.clone(),
        minimum_revision: 2,
        expires_at: apsp_protocol::now_timestamp() + 60,
    });
    state.enqueue_runtime_control(&request).unwrap();
    let result = ControlResult::Barrier(RuntimeRecoveryBarrierResult {
        request_id: request.id(),
        request_digest: request.digest().unwrap(),
        observed: Some(expected),
        minimum_revision: Some(2),
        pending_intents_clear: true,
        success: true,
        error: None,
    });
    let intent = crate::state::IntentRecord {
        op_id: Uuid::new_v4(),
        module: "demo".into(),
        payload: serde_json::json!({}),
    };
    state.begin_intent(&intent).unwrap();
    assert!(
        state
            .finish_runtime_control(&result, Some(("demo", 2)))
            .is_err()
    );
    assert_eq!(state.runtime_revision_floor("demo").unwrap(), 0);
    assert!(state.runtime_control_result(&request).unwrap().is_none());
    state.finish_intent(intent.op_id).unwrap();
    state
        .finish_runtime_control(&result, Some(("demo", 2)))
        .unwrap();
    assert_eq!(state.runtime_revision_floor("demo").unwrap(), 2);
    assert_eq!(
        state.runtime_control_result(&request).unwrap(),
        Some(result.clone())
    );
    state
        .finish_runtime_control(&result, Some(("demo", 2)))
        .unwrap();
    assert_eq!(state.runtime_revision_floor("demo").unwrap(), 2);
}

#[test]
fn additive_runtime_tables_preserve_legacy_state_and_pending_intents() {
    let mut state = State::open(std::path::Path::new(":memory:")).unwrap();
    state
        .set_json("applied:demo", &serde_json::json!({"legacy": true}))
        .unwrap();
    state
        .connection
        .execute_batch("DROP TABLE runtime_control; DROP TABLE runtime_revision_floors;")
        .unwrap();
    state
        .connection
        .execute_batch(include_str!("../migrations/0005_runtime_control.sql"))
        .unwrap();
    assert_eq!(
        state.get_json::<serde_json::Value>("applied:demo").unwrap(),
        Some(serde_json::json!({"legacy": true}))
    );
    assert_eq!(state.runtime_revision_floor("demo").unwrap(), 0);
    assert!(state.pending_runtime_controls().unwrap().is_empty());
}

#![forbid(unsafe_code)]

use apsp_protocol::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

fn binding() -> RuntimeBinding {
    RuntimeBinding::new(Uuid::from_u128(1), "runtime".into(), 7, "a".repeat(64))
}

fn checkpoint() -> RuntimeCheckpoint {
    RuntimeCheckpoint {
        binding: binding(),
        activation_id: Uuid::from_u128(2),
        instance_id: "backend:instance-1".into(),
        healthy: true,
    }
}

fn request() -> RuntimeCheckpointRequest {
    RuntimeCheckpointRequest {
        request_id: Uuid::from_u128(3),
        expected: binding(),
        expires_at: 1_800_000_120,
    }
}

fn barrier() -> RuntimeRecoveryBarrierRequest {
    RuntimeRecoveryBarrierRequest {
        request_id: Uuid::from_u128(4),
        expected: checkpoint(),
        minimum_revision: 6,
        expires_at: 1_800_000_120,
    }
}

fn probe() -> RuntimePathProbeRequest {
    RuntimePathProbeRequest {
        request_id: Uuid::from_u128(5),
        expected: checkpoint(),
        probe_id: Uuid::from_u128(6),
        expires_at: 1_800_000_120,
    }
}

#[test]
fn probe_wire_roundtrips_and_digest_binds_the_entire_applied_activation() {
    let original = probe();
    let result = RuntimePathProbeResult {
        request_id: original.request_id,
        request_digest: original.digest().unwrap(),
        observed: Some(original.expected.clone()),
        probe_id: original.probe_id,
        elapsed_ms: Some(17),
        success: true,
        error: None,
    };
    for message in [
        Message::RuntimePathProbeRequest(original.clone()),
        Message::RuntimePathProbeResult(result),
        Message::RuntimePathProbeAck(RuntimeControlAck {
            request_id: original.request_id,
            request_digest: original.digest().unwrap(),
        }),
    ] {
        assert_eq!(
            message.clone().into_envelope().unwrap().decode().unwrap(),
            message
        );
    }
    let mut variants = Vec::new();
    let mut changed = original.clone();
    changed.probe_id = Uuid::from_u128(99);
    variants.push(changed);
    let mut changed = original.clone();
    changed.expected.activation_id = Uuid::from_u128(99);
    variants.push(changed);
    let mut changed = original.clone();
    changed.expected.instance_id = "backend:another".into();
    variants.push(changed);
    let mut changed = original.clone();
    changed.expected.binding.deployment_id = Uuid::from_u128(99);
    variants.push(changed);
    let mut changed = original.clone();
    changed.expires_at += 1;
    variants.push(changed);
    for changed in variants {
        assert_ne!(original.digest().unwrap(), changed.digest().unwrap());
    }
    let mut wire = json!(original);
    wire["url"] = json!("https://arbitrary.example");
    assert!(serde_json::from_value::<RuntimePathProbeRequest>(wire).is_err());
}

#[test]
fn probe_deadlines_and_result_shape_fail_closed() {
    let original = probe();
    assert!(original.valid_at(1_800_000_000));
    assert!(!original.valid_at(1_799_999_999));
    assert!(!original.valid_at(original.expires_at));
    let mut invalid = original.clone();
    invalid.probe_id = Uuid::nil();
    assert!(!invalid.valid());
    let mut invalid = original.clone();
    invalid.expected.healthy = false;
    assert!(!invalid.valid());
    let mut result = RuntimePathProbeResult {
        request_id: original.request_id,
        request_digest: original.digest().unwrap(),
        observed: Some(original.expected),
        probe_id: original.probe_id,
        elapsed_ms: Some(17),
        success: true,
        error: None,
    };
    assert!(result.valid());
    for measurement in [None, Some(0), Some(5001)] {
        result.elapsed_ms = measurement;
        assert!(!result.valid());
    }
    result.success = false;
    result.error = Some("failed".into());
    result.elapsed_ms = Some(17);
    assert!(!result.valid());
    result.elapsed_ms = None;
    assert!(result.valid());
}

#[test]
fn runtime_control_messages_roundtrip_without_changing_legacy_apply() {
    let request = request();
    let barrier = barrier();
    let ack = RuntimeControlAck {
        request_id: request.request_id,
        request_digest: request.digest().unwrap(),
    };
    let messages = [
        Message::RuntimeCheckpointRequest(request.clone()),
        Message::RuntimeCheckpointResult(RuntimeCheckpointResult {
            request_id: request.request_id,
            request_digest: request.digest().unwrap(),
            observed: Some(checkpoint()),
            success: true,
            error: None,
        }),
        Message::RuntimeCheckpointAck(ack.clone()),
        Message::RuntimeRecoveryBarrierRequest(barrier.clone()),
        Message::RuntimeRecoveryBarrierResult(RuntimeRecoveryBarrierResult {
            request_id: barrier.request_id,
            request_digest: barrier.digest().unwrap(),
            observed: Some(checkpoint()),
            minimum_revision: Some(6),
            pending_intents_clear: true,
            success: true,
            error: None,
        }),
        Message::RuntimeRecoveryBarrierAck(ack),
    ];
    for message in messages {
        let envelope = message.clone().into_envelope().unwrap();
        assert_eq!(envelope.message_type, message.message_type());
        let envelope: Envelope =
            serde_json::from_slice(&serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert_eq!(envelope.decode().unwrap(), message);
    }
    let apply = ApplyResult {
        module: "runtime".into(),
        rev: 7,
        op_id: Uuid::from_u128(9),
        status: ApplyStatus::Applied,
        healthy: true,
        error: None,
    };
    assert_eq!(
        serde_json::to_value(&apply).unwrap(),
        json!({"module":"runtime","rev":7,"op_id":Uuid::from_u128(9),"status":"applied","healthy":true})
    );
    assert_eq!(
        Message::ApplyResult(apply.clone())
            .into_envelope()
            .unwrap()
            .decode()
            .unwrap(),
        Message::ApplyResult(apply)
    );
}

#[test]
fn request_digest_binds_kind_and_every_checkpoint_input() {
    let original = request();
    let digest = original.digest().unwrap();
    assert_eq!(digest, original.clone().digest().unwrap());
    let mut expected = Sha256::new();
    expected.update(b"apsp-runtime-control-v1\0runtime.checkpoint.request\0");
    expected.update(serde_json::to_vec(&original).unwrap());
    assert_eq!(digest, format!("{:x}", expected.finalize()));
    let mut other_kind = Sha256::new();
    other_kind.update(b"apsp-runtime-control-v1\0runtime.barrier.request\0");
    other_kind.update(serde_json::to_vec(&original).unwrap());
    assert_ne!(digest, format!("{:x}", other_kind.finalize()));
    let mut changed = original.clone();
    changed.request_id = Uuid::from_u128(99);
    assert_ne!(digest, changed.digest().unwrap());
    let mut changed = original.clone();
    changed.expires_at += 1;
    assert_ne!(digest, changed.digest().unwrap());
    for expected in [
        RuntimeBinding::new(Uuid::from_u128(99), "runtime".into(), 7, "a".repeat(64)),
        RuntimeBinding::new(Uuid::from_u128(1), "other".into(), 7, "a".repeat(64)),
        RuntimeBinding::new(Uuid::from_u128(1), "runtime".into(), 8, "a".repeat(64)),
        RuntimeBinding::new(Uuid::from_u128(1), "runtime".into(), 7, "b".repeat(64)),
    ] {
        let mut changed = original.clone();
        changed.expected = expected;
        assert_ne!(digest, changed.digest().unwrap());
    }
    let mut changed = original;
    changed.expected.binding_digest = "b".repeat(64);
    assert_ne!(digest, changed.digest().unwrap());
}

#[test]
fn barrier_digest_binds_activation_instance_floor_and_health() {
    let original = barrier();
    let digest = original.digest().unwrap();
    let mut changed = original.clone();
    changed.expected.activation_id = Uuid::from_u128(99);
    assert_ne!(digest, changed.digest().unwrap());
    let mut changed = original.clone();
    changed.expected.instance_id = "backend:instance-2".into();
    assert_ne!(digest, changed.digest().unwrap());
    let mut changed = original.clone();
    changed.expected.healthy = false;
    assert_ne!(digest, changed.digest().unwrap());
    let mut changed = original.clone();
    changed.minimum_revision = 7;
    assert_ne!(digest, changed.digest().unwrap());
    let mut changed = original.clone();
    changed.expires_at += 1;
    assert_ne!(digest, changed.digest().unwrap());
    let mut changed = original;
    changed.request_id = Uuid::from_u128(99);
    assert_ne!(digest, changed.digest().unwrap());
}

#[test]
fn identities_are_bounded_and_a_supplied_binding_digest_is_not_trusted() {
    assert!(binding().valid());
    let mut value = binding();
    value.deployment_id = Uuid::nil();
    assert!(!value.valid());
    let mut value = binding();
    value.bundle_sha256 = "A".repeat(64);
    value.binding_digest = value.computed_binding_digest();
    assert!(!value.valid());
    let mut value = binding();
    value.revision = 0;
    value.binding_digest = value.computed_binding_digest();
    assert!(!value.valid());
    let mut value = binding();
    value.binding_digest = "b".repeat(64);
    assert!(!value.valid());
    let mut value = binding();
    value.revision = i64::MAX as u64 + 1;
    value.binding_digest = value.computed_binding_digest();
    assert!(!value.valid());
    for module in [
        "",
        "../runtime",
        "runtime/name",
        "runtime\n",
        &"a".repeat(65),
    ] {
        assert!(!runtime_module_valid(module));
    }
    assert!(runtime_module_valid("a-runtime.v1_2"));
    let mut value = checkpoint();
    value.instance_id = "a".repeat(257);
    assert!(!value.valid());
    let mut value = checkpoint();
    value.instance_id = "instance\n".into();
    assert!(!value.valid());
    let mut value = checkpoint();
    value.activation_id = Uuid::nil();
    assert!(!value.valid());
}

#[test]
fn deadline_and_unknown_fields_are_rejected() {
    let now = 1_800_000_000;
    let mut request = request();
    assert!(request.valid_at(now));
    request.expires_at = now;
    assert!(!request.valid_at(now));
    request.expires_at = now + 600;
    assert!(request.valid_at(now));
    request.expires_at += 1;
    assert!(!request.valid_at(now));
    request.request_id = Uuid::nil();
    assert!(!request.valid());
    let mut wire = json!(barrier());
    wire["unexpected"] = json!(true);
    assert!(serde_json::from_value::<RuntimeRecoveryBarrierRequest>(wire).is_err());
    let mut wire = json!(binding());
    wire["path"] = json!("/untrusted/config");
    assert!(serde_json::from_value::<RuntimeBinding>(wire).is_err());
    let mut barrier = barrier();
    barrier.minimum_revision = 8;
    assert!(!barrier.valid());
    barrier.minimum_revision = 0;
    assert!(!barrier.valid());
    barrier.minimum_revision = 7;
    barrier.expected.healthy = false;
    assert!(!barrier.valid());
}

#[test]
fn success_requires_a_healthy_observation_and_failure_requires_a_bounded_reason() {
    let request = request();
    let mut value = RuntimeCheckpointResult {
        request_id: request.request_id,
        request_digest: request.digest().unwrap(),
        observed: Some(checkpoint()),
        success: true,
        error: None,
    };
    assert!(value.valid());
    value.observed = None;
    assert!(!value.valid());
    value.observed = Some(checkpoint());
    value.error = Some("unexpected".into());
    assert!(!value.valid());
    value.error = None;
    value.observed.as_mut().unwrap().healthy = false;
    assert!(!value.valid());
    value.success = false;
    assert!(!value.valid());
    value.error = Some("需要管理员新部署后重新认证".into());
    assert!(value.valid());
    value.error = Some("x".repeat(1025));
    assert!(!value.valid());
    value.error = Some("bad\nreason".into());
    assert!(!value.valid());
    value.error = Some("reason".into());
    value.request_digest = "x".repeat(64);
    assert!(!value.valid());
}

#[test]
fn successful_barrier_requires_a_durable_floor_and_cleared_intents() {
    let request = barrier();
    let mut value = RuntimeRecoveryBarrierResult {
        request_id: request.request_id,
        request_digest: request.digest().unwrap(),
        observed: Some(checkpoint()),
        minimum_revision: Some(6),
        pending_intents_clear: true,
        success: true,
        error: None,
    };
    assert!(value.valid());
    value.pending_intents_clear = false;
    assert!(!value.valid());
    value.pending_intents_clear = true;
    value.minimum_revision = None;
    assert!(!value.valid());
    value.minimum_revision = Some(8);
    assert!(!value.valid());
    value.minimum_revision = Some(0);
    assert!(!value.valid());
    value.success = false;
    value.minimum_revision = None;
    value.error = Some("pending intent remains".into());
    assert!(value.valid());
    let mut ack = RuntimeControlAck {
        request_id: request.request_id,
        request_digest: request.digest().unwrap(),
    };
    assert!(ack.valid());
    ack.request_id = Uuid::nil();
    assert!(!ack.valid());
}

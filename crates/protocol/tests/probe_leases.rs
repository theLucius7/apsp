#![forbid(unsafe_code)]

use apsp_protocol::{
    AuthorizedProbe, ProbeAddressFamily, ProbeAuthorization, ProbeAuthorizationKind,
    ProbeExecution, ProbeLease, ProbeMonitor, ProbeNetwork, ProbeResult, ProbeSpec,
};
use serde_json::json;
use uuid::Uuid;

const LEGACY_SPEC: &str = r#"{"id":"00000000-0000-0000-0000-000000000003","name":"fixture","kind":"tcp","target":"127.0.0.1","port":443,"interval_secs":10,"carrier":"","enabled":true}"#;
const LEGACY_RESULT: &str = r#"{"id":"00000000-0000-0000-0000-000000000004","probe_id":"00000000-0000-0000-0000-000000000003","sampled_at":1790000000000,"latency_ms":0.0,"loss_percent":0.0,"error":null}"#;

fn lease() -> ProbeLease {
    let mut spec: ProbeSpec = serde_json::from_str(LEGACY_SPEC).unwrap();
    let authorization = ProbeAuthorization {
        kind: ProbeAuthorizationKind::Owned,
        source: "TEST_ONLY operator-owned loopback".into(),
        scope: "Only this fixture endpoint and method".into(),
        enabled: true,
        expires_at: None,
        identity: spec.identity(),
    };
    spec.monitor = Some(ProbeMonitor {
        network: ProbeNetwork::Other,
        region: "fixture region".into(),
        address_family: ProbeAddressFamily::Any,
        authorization: Some(authorization.clone()),
    });
    ProbeLease {
        id: Uuid::from_u128(5),
        server_id: 7,
        revision: 1,
        issued_at: 1_790_000_000,
        expires_at: 1_790_000_090,
        probes: vec![AuthorizedProbe {
            spec,
            authorization,
        }],
    }
}

#[test]
fn legacy_probe_vector_and_results_keep_their_exact_wire_bytes() {
    let spec: ProbeSpec = serde_json::from_str(LEGACY_SPEC).unwrap();
    assert!(spec.valid());
    assert!(!spec.authorized_at(1_790_000_000));
    assert_eq!(
        serde_json::to_string(&vec![spec]).unwrap(),
        format!("[{LEGACY_SPEC}]")
    );
    let result: ProbeResult = serde_json::from_str(LEGACY_RESULT).unwrap();
    assert!(result.execution.is_none());
    assert_eq!(serde_json::to_string(&result).unwrap(), LEGACY_RESULT);
}

#[test]
fn lease_lifetime_device_identity_and_revision_limits_are_checked() {
    let original = lease();
    assert!(original.valid());
    assert_eq!(apsp_protocol::MAX_PROBE_LEASE_SECS, 90);
    for seconds in [1, 90] {
        let mut changed = original.clone();
        changed.expires_at = changed.issued_at + seconds;
        assert!(changed.valid());
    }
    for seconds in [-1, 0, 91] {
        let mut changed = original.clone();
        changed.expires_at = changed.issued_at + seconds;
        assert!(!changed.valid());
    }
    for (field, value) in [
        ("id", json!(Uuid::nil())),
        ("server_id", json!(0)),
        ("revision", json!(u64::MAX)),
        ("issued_at", json!(0)),
    ] {
        let mut changed = json!(original);
        changed[field] = value;
        assert!(
            !serde_json::from_value::<ProbeLease>(changed)
                .unwrap()
                .valid(),
            "{field}"
        );
    }
}

#[test]
fn exact_main_monitor_authorization_is_required_and_cannot_be_retargeted() {
    for property in ["source", "scope", "enabled", "target", "expiry", "monitor"] {
        let mut changed = lease();
        let probe = &mut changed.probes[0];
        match property {
            "source" => probe.authorization.source = "other source".into(),
            "scope" => probe.authorization.scope = "other scope".into(),
            "enabled" => probe.spec.enabled = false,
            "target" => probe.spec.target = "127.0.0.2".into(),
            "expiry" => probe.authorization.expires_at = Some(changed.expires_at - 1),
            _ => probe.spec.monitor = None,
        }
        assert!(!changed.valid(), "{property}");
    }
    let mut changed = lease();
    changed.probes[0].authorization.expires_at = Some(changed.expires_at);
    changed.probes[0]
        .spec
        .monitor
        .as_mut()
        .unwrap()
        .authorization = Some(changed.probes[0].authorization.clone());
    assert!(changed.valid());
    changed.expires_at += 1;
    assert!(!changed.valid());
}

#[test]
fn duplicate_targets_bounds_unknown_fields_and_execution_context_are_checked() {
    let mut changed = lease();
    changed.probes.push(changed.probes[0].clone());
    assert!(!changed.valid());
    changed.probes.clear();
    assert!(changed.valid());
    let original = lease();
    let execution = ProbeExecution {
        lease_id: original.id,
        revision: original.revision,
        issued_at: original.issued_at,
        expires_at: original.expires_at,
        probe: original.probes[0].clone(),
    };
    assert!(execution.valid());
    for field in ["lease", "probe", "authorization", "execution"] {
        let mut wire = json!(original);
        match field {
            "lease" => wire["unexpected"] = json!(true),
            "probe" => wire["probes"][0]["unexpected"] = json!(true),
            "authorization" => wire["probes"][0]["authorization"]["unexpected"] = json!(true),
            _ => {
                let mut wire = json!(execution);
                wire["unexpected"] = json!(true);
                assert!(serde_json::from_value::<ProbeExecution>(wire).is_err());
                continue;
            }
        }
        assert!(
            serde_json::from_value::<ProbeLease>(wire).is_err(),
            "{field}"
        );
    }
    changed = original.clone();
    changed.probes = (1..=33)
        .map(|id| {
            let mut probe = original.probes[0].clone();
            probe.spec.id = Uuid::from_u128(id);
            probe
        })
        .collect();
    assert!(!changed.valid());
}

// Keep the author's boundary cases on the established monitor authorization wire.
#[test]
fn authorization_bounds_and_expiry_apply_to_the_exact_monitor_identity() {
    for kind in [
        ProbeAuthorizationKind::Owned,
        ProbeAuthorizationKind::Consent,
    ] {
        let mut spec = lease().probes.remove(0).spec;
        let permission = spec
            .monitor
            .as_mut()
            .unwrap()
            .authorization
            .as_mut()
            .unwrap();
        permission.kind = kind;
        permission.expires_at = Some(200);
        assert!(spec.authorized_at(199));
        assert!(!spec.authorized_at(200));
        assert!(!spec.authorized_at(201));
        for (field, value) in [
            ("source", json!("   ")),
            ("scope", json!("   ")),
            ("source", json!("s".repeat(257))),
            ("scope", json!("e".repeat(513))),
            ("source", json!("source\ncontrol")),
            ("scope", json!("scope\0control")),
            ("expires_at", json!(0)),
            ("expires_at", json!(-1)),
        ] {
            let mut wire = json!(spec);
            wire["monitor"]["authorization"][field] = value;
            assert!(
                !serde_json::from_value::<ProbeSpec>(wire).unwrap().valid(),
                "{field}"
            );
        }
    }
    let mut wire: serde_json::Value = serde_json::from_str(LEGACY_SPEC).unwrap();
    wire["authorization"] = json!(lease().probes[0].authorization);
    assert!(serde_json::from_value::<ProbeSpec>(wire).is_err());
}

#[test]
fn all_lease_control_fields_are_required_and_empty_replacement_remains_valid() {
    let original = lease();
    for field in [
        "id",
        "server_id",
        "revision",
        "issued_at",
        "expires_at",
        "probes",
    ] {
        let mut wire = json!(original);
        wire.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<ProbeLease>(wire).is_err(),
            "{field}"
        );
    }
    let mut changed = original.clone();
    changed.revision = i64::MAX as u64;
    assert!(changed.valid());
    changed.issued_at = i64::MIN;
    changed.expires_at = i64::MAX;
    assert!(!changed.valid());
    changed = original.clone();
    changed.probes = (1..=32)
        .map(|id| {
            let mut probe = original.probes[0].clone();
            probe.spec.id = Uuid::from_u128(id);
            probe
        })
        .collect();
    assert!(changed.valid());
    changed.probes.clear();
    assert!(
        changed.valid(),
        "an empty replacement must stop all leased work"
    );
}

#[test]
fn execution_records_round_trip_the_exact_bound_authorization_and_measurement_method() {
    let snapshot = lease();
    let execution = ProbeExecution {
        lease_id: snapshot.id,
        revision: snapshot.revision,
        issued_at: snapshot.issued_at,
        expires_at: snapshot.expires_at,
        probe: snapshot.probes[0].clone(),
    };
    let mut result: ProbeResult = serde_json::from_str(LEGACY_RESULT).unwrap();
    result.execution = Some(execution.clone());
    let wire = json!(result);
    assert_eq!(wire["execution"]["probe"]["authorization"]["kind"], "owned");
    assert_eq!(serde_json::from_value::<ProbeResult>(wire).unwrap(), result);
    let mut changed = execution;
    changed.probe.spec.kind = apsp_protocol::ProbeKind::Icmp;
    assert!(
        !changed.valid(),
        "a TCP port and identity must never authorize ICMP"
    );
}

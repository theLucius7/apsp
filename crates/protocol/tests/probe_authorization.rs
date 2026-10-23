#![forbid(unsafe_code)]

use apsp_protocol::{
    ProbeAddressFamily, ProbeAuthorization, ProbeAuthorizationKind, ProbeMonitor, ProbeSpec,
};
use serde_json::json;

fn legacy() -> ProbeSpec {
    serde_json::from_value(json!({"id":uuid::Uuid::nil(),"name":"owned endpoint","kind":"tcp","target":"probe.example.com","port":443,"interval_secs":30,"carrier":"telecom","enabled":true})).unwrap()
}

fn granted() -> ProbeSpec {
    let mut spec = legacy();
    let identity = spec.identity();
    spec.monitor = Some(ProbeMonitor {
        network: apsp_protocol::ProbeNetwork::Other,
        region: "fixture region".into(),
        address_family: ProbeAddressFamily::Any,
        authorization: Some(ProbeAuthorization {
            kind: ProbeAuthorizationKind::Owned,
            source: "TEST_ONLY owner inventory record".into(),
            scope: "Only the exact endpoint, method, port and family".into(),
            enabled: true,
            expires_at: Some(200),
            identity,
        }),
    });
    spec
}

#[test]
fn historical_probes_remain_readable_without_implicit_permission() {
    let mut spec = legacy();
    assert!(spec.valid());
    assert!(!spec.runnable_at(100));
    let encoded = serde_json::to_value(&spec).unwrap();
    assert!(encoded.get("monitor").is_none());
    assert!(encoded.get("execution_authorized").is_none());
    spec.execution_authorized = Some(true);
    assert!(!spec.runnable_at(100));
}

#[test]
fn permission_binds_exact_identity_and_expires_at_the_boundary() {
    let spec = granted();
    assert!(spec.runnable_at(199));
    assert!(!spec.runnable_at(200));
    for property in ["kind", "target", "port", "address_family"] {
        let mut changed = serde_json::to_value(&spec).unwrap();
        let identity = &mut changed["monitor"]["authorization"]["identity"];
        identity[property] = match property {
            "kind" => json!("icmp"),
            "target" => json!("different.example.com"),
            "port" => json!(8443),
            _ => json!("ipv6"),
        };
        let changed: ProbeSpec = serde_json::from_value(changed).unwrap();
        assert!(!changed.valid(), "{property}");
        assert!(!changed.runnable_at(100), "{property}");
    }
}

#[test]
fn revocation_and_invalid_provenance_fail_closed() {
    let spec = granted();
    for value in ["", "\nsource", " source "] {
        let mut changed = spec.clone();
        changed
            .monitor
            .as_mut()
            .unwrap()
            .authorization
            .as_mut()
            .unwrap()
            .source = value.into();
        assert!(!changed.runnable_at(100));
    }
    let mut revoked = spec.clone();
    revoked
        .monitor
        .as_mut()
        .unwrap()
        .authorization
        .as_mut()
        .unwrap()
        .enabled = false;
    assert!(!revoked.runnable_at(100));
    let mut malformed = spec;
    malformed
        .monitor
        .as_mut()
        .unwrap()
        .authorization
        .as_mut()
        .unwrap()
        .expires_at = Some(i64::MAX);
    assert!(!malformed.valid());
}

#[test]
fn family_and_metadata_identity_never_relabel_existing_samples() {
    let original = granted();
    let mut changed = original.clone();
    changed.carrier = "unicom".into();
    assert!(!changed.same_measurement_identity(&original));
    changed = original.clone();
    changed.monitor.as_mut().unwrap().region = "other region".into();
    assert!(!changed.same_measurement_identity(&original));
    changed = original.clone();
    changed.monitor.as_mut().unwrap().network = apsp_protocol::ProbeNetwork::Mobile;
    assert!(!changed.same_measurement_identity(&original));
    changed = original.clone();
    changed.interval_secs = 60;
    changed.name = "renamed endpoint".into();
    assert!(changed.same_measurement_identity(&original));
    assert!(ProbeAddressFamily::Ipv4.allows("127.0.0.1".parse().unwrap()));
    assert!(!ProbeAddressFamily::Ipv4.allows("::1".parse().unwrap()));
    assert!(ProbeAddressFamily::Ipv6.allows("::1".parse().unwrap()));
}

#[test]
fn legacy_results_omit_new_attempts_and_family_fields_without_changing_digest_input() {
    let input = json!({"id":uuid::Uuid::nil(),"probe_id":uuid::Uuid::nil(),"sampled_at":1000,"latency_ms":null,"loss_percent":100.0,"error":"TEST_ONLY old unavailable result"});
    let result: apsp_protocol::ProbeResult = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(result.attempts, None);
    assert_eq!(result.address_family, None);
    assert_eq!(serde_json::to_value(result).unwrap(), input);
}

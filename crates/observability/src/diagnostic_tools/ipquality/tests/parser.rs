use super::super::*;
use super::fixtures::{HASH, IP, body, parse_body};
use serde_json::{Value, json};
use uuid::Uuid;

#[test]
fn missing_raw_json_preserves_valid_transport_receipts_without_creating_known_fields() {
    for complete in [false, true] {
        let mut value = body(Uuid::new_v4(), 1000);
        value["upstream"] = Value::Null;
        if !complete {
            value["finished_at"] = Value::Null;
        }
        value["attempts"].as_array_mut().unwrap().push(json!({
            "seq":3,"provider":"youtube-public-page","dataset":"Youtube","target_ip":IP,
            "url":"https://www.youtube.com/premium","status":"failed","attempted_at":1000,
            "elapsed_ms":3,"http_status":403,"curl_exit":0,"response_bytes":1,
            "error_kind":"http_403","error_message":"TEST_ONLY source denied request"
        }));
        let projection = parse_body(value, 1000, complete).unwrap();
        assert_eq!(projection.egress_ip.as_deref(), Some(IP));
        assert_eq!(projection.quality.len(), 2);
        let datasets: Vec<_> = projection
            .quality
            .iter()
            .flat_map(|entry| &entry.databases)
            .collect();
        assert!(
            datasets
                .iter()
                .all(|dataset| dataset.status == "failed" && dataset.fields.is_empty())
        );
        assert_eq!(
            datasets
                .iter()
                .find(|dataset| dataset.database == "node-ipapi")
                .unwrap()
                .error_kind,
            Some(crate::ip_quality::QueryErrorKind::SchemaMismatch)
        );
        assert_eq!(
            datasets
                .iter()
                .find(|dataset| dataset.database == "node-Youtube")
                .unwrap()
                .error_kind,
            Some(crate::ip_quality::QueryErrorKind::Http403)
        );
    }
}

#[test]
fn unobserved_exit_does_not_allow_invalid_or_private_request_targets() {
    for target in ["192.0.2.1", "invalid", "2001:db9:0::1"] {
        let mut value = body(Uuid::new_v4(), 1000);
        value["egress_ip"] = Value::Null;
        value["upstream"] = Value::Null;
        value["attempts"].as_array_mut().unwrap().truncate(1);
        let receipt = &mut value["attempts"][0];
        receipt["target_ip"] = json!(target);
        receipt["status"] = json!("failed");
        receipt["http_status"] = Value::Null;
        receipt["curl_exit"] = json!(6);
        receipt["error_kind"] = json!("dns");
        receipt["error_message"] = json!("TEST_ONLY discovery failed");
        assert!(parse_body(value, 1000, true).is_err(), "{target}");
    }
}

#[test]
fn parameters_are_a_single_explicit_family_without_shell_or_credentials() {
    for value in [json!({"ip_version":"4"}), json!({"ip_version":"6"})] {
        assert!(serde_json::from_value::<Request>(value).unwrap().valid());
    }
    for value in [
        json!({}),
        json!({"ip_version":"both"}),
        json!({"ip_version":4}),
        json!({"ip_version":"6","command":"fixture"}),
        json!({"ip_version":"6","api_key":"TEST_ONLY"}),
    ] {
        assert!(serde_json::from_value::<Request>(value).map_or(true, |request| !request.valid()));
    }
}

#[test]
fn zero_and_false_require_complete_request_identity_and_remain_raw_values() {
    let projection = parse_body(body(Uuid::new_v4(), 1000), 1000, true).unwrap();
    assert_eq!(projection.egress_ip.as_deref(), Some(IP));
    let entry = &projection.quality[0];
    assert_eq!(entry.provider, "ipquality-node/check-place-aggregator");
    assert_eq!(entry.databases.len(), 1);
    let dataset = &entry.databases[0];
    assert_eq!(dataset.status, "succeeded");
    assert_eq!(dataset.execution, "node");
    assert_eq!(dataset.observed_ip.as_deref(), Some(IP));
    assert!(dataset.source.is_none());
    assert!(
        dataset
            .fields
            .iter()
            .any(|field| field.value.as_bool() == Some(false))
    );
    assert!(
        dataset
            .fields
            .iter()
            .any(|field| field.value.as_f64() == Some(0.0))
    );
    assert!(
        !dataset
            .fields
            .iter()
            .any(|field| field.label.contains("干净"))
    );
}

#[test]
fn a_completed_dataset_survives_an_active_partial_chapter_without_inventing_missing_sources() {
    let mut value = body(Uuid::new_v4(), 1000);
    value["finished_at"] = Value::Null;
    let partial = parse_body(value.clone(), 1000, false).unwrap();
    assert_eq!(partial.quality.len(), 1);
    assert_eq!(partial.quality[0].databases[0].status, "succeeded");
    assert!(parse_body(value, 1000, true).is_err());
}

#[test]
fn transport_failure_is_unknown_even_if_the_raw_json_has_a_zero_score() {
    for (kind, status, curl_exit) in [
        ("http_403", Some(403), 0),
        ("http_429", Some(429), 0),
        ("timeout", None, 28),
        ("dns", None, 6),
        ("connect", None, 7),
        ("tls", None, 60),
        ("non_json", Some(200), 0),
        ("schema_mismatch", Some(200), 0),
    ] {
        let mut value = body(Uuid::new_v4(), 1000);
        let receipt = &mut value["attempts"][1];
        receipt["status"] = json!("failed");
        receipt["error_kind"] = json!(kind);
        receipt["error_message"] = json!("TEST_ONLY unavailable response");
        receipt["http_status"] = json!(status);
        receipt["curl_exit"] = json!(curl_exit);
        let parsed = parse_body(value, 1000, true).unwrap();
        assert_eq!(parsed.quality[0].databases[0].status, "failed", "{kind}");
        assert!(parsed.quality[0].databases[0].fields.is_empty(), "{kind}");
    }
}

#[test]
fn fixed_source_artifact_family_raw_identity_and_request_ledger_are_required() {
    let initial = body(Uuid::new_v4(), 1000);
    for (pointer, replacement) in [
        ("/version", json!("online-main")),
        ("/source_commit", json!("main")),
        ("/source_sha256", json!(HASH)),
        (
            "/artifact_sha256",
            json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        ),
        ("/ip_version", json!("4")),
        ("/egress_ip", json!("2001:db8::1")),
        ("/upstream/Head/IP", json!("2001:db9::2")),
        ("/upstream/Head/Version", json!("unknown")),
        ("/attempts/1/target_ip", json!("2001:db9::2")),
        ("/attempts/1/seq", json!(1)),
        ("/attempts/1/provider", json!("fixture-provider")),
        ("/attempts/1/url", json!("https://example.test/fixture")),
        ("/attempts/1/http_status", json!(403)),
        ("/attempts/1/curl_exit", json!(6)),
        ("/attempts/1/response_bytes", json!(0)),
        ("/attempts/1/attempted_at", json!(999)),
        ("/started_at", json!(699)),
        ("/finished_at", json!(1601)),
    ] {
        let mut changed = initial.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        assert!(parse_body(changed, 1000, true).is_err(), "{pointer}");
    }
    let mut changed = initial;
    changed["attempts"][1]
        .as_object_mut()
        .unwrap()
        .remove("error_kind");
    assert!(parse_body(changed, 1000, true).is_err());
}

#[test]
fn absent_or_ill_typed_upstream_fields_do_not_create_success() {
    for fields in [
        json!({}),
        json!({"Score":{"ipapi":false},"Factor":{"Proxy":{"ipapi":"false"}}}),
        json!({"Score":{"ipapi":"null"},"Factor":{"Proxy":{"ipapi":null}}}),
    ] {
        let mut value = body(Uuid::new_v4(), 1000);
        value["upstream"] = fields;
        value["upstream"]["Head"] = json!({"IP":IP,"Version":"v2026-09-16"});
        let parsed = parse_body(value, 1000, true).unwrap();
        let dataset = &parsed.quality[0].databases[0];
        assert!(dataset.fields.is_empty());
        assert_eq!(
            dataset.error_kind,
            Some(crate::ip_quality::QueryErrorKind::SchemaMismatch)
        );
    }
}

#[test]
fn discovery_failure_does_not_associate_an_unknown_result_with_a_nic_address() {
    let mut value = body(Uuid::new_v4(), 1000);
    value["egress_ip"] = Value::Null;
    value["upstream"] = Value::Null;
    value["attempts"].as_array_mut().unwrap().truncate(1);
    let receipt = &mut value["attempts"][0];
    receipt["target_ip"] = Value::Null;
    receipt["status"] = json!("failed");
    receipt["error_kind"] = json!("dns");
    receipt["error_message"] = json!("TEST_ONLY discovery DNS failed");
    receipt["http_status"] = Value::Null;
    receipt["curl_exit"] = json!(6);
    let parsed = parse_body(value, 1000, true).unwrap();
    assert!(parsed.egress_ip.is_none());
    assert!(parsed.quality.is_empty());
}

#[test]
fn the_same_endpoint_cannot_be_retried_and_rejections_cannot_be_reclassified_as_success() {
    let initial = body(Uuid::new_v4(), 1000);
    let mut duplicate = initial.clone();
    let mut third = duplicate["attempts"][1].clone();
    third["seq"] = json!(3);
    duplicate["attempts"].as_array_mut().unwrap().push(third);
    assert!(parse_body(duplicate, 1000, true).is_err());
    for (kind, status) in [("http_403", 429), ("http_429", 403), ("request_error", 403)] {
        let mut changed = initial.clone();
        changed["attempts"][1]["status"] = json!("failed");
        changed["attempts"][1]["error_kind"] = json!(kind);
        changed["attempts"][1]["error_message"] = json!("TEST_ONLY inconsistent denial");
        changed["attempts"][1]["http_status"] = json!(status);
        assert!(parse_body(changed, 1000, true).is_err());
    }
}

#[test]
fn no_credentials_or_disabled_probe_is_sent_as_a_query_receipt() {
    let mut value = body(Uuid::new_v4(), 1000);
    value["attempts"].as_array_mut().unwrap().push(json!({"seq":3,"provider":"openai-not-configured","dataset":"OpenAI","target_ip":IP,"url":null,"status":"not_attempted","attempted_at":null,"elapsed_ms":null,"http_status":null,"curl_exit":null,"response_bytes":null,"error_kind":"not_attempted","error_message":"TEST_ONLY no authorized credential adapter"}));
    let parsed = parse_body(value.clone(), 1000, true).unwrap();
    let skipped = parsed
        .quality
        .iter()
        .find(|entry| entry.provider.ends_with("openai-not-configured"))
        .unwrap();
    assert!(skipped.databases[0].fields.is_empty());
    assert!(skipped.databases[0].attempted_at.is_none());
    assert_eq!(
        skipped.databases[0].error_kind,
        Some(crate::ip_quality::QueryErrorKind::NotAttempted)
    );
    value["attempts"][2]["url"] = json!("https://example.test/should-not-run");
    assert!(parse_body(value, 1000, true).is_err());
}

#[test]
fn typed_saved_node_fields_do_not_gain_meaning_from_unknown_labels_or_types() {
    let projection = parse_body(body(Uuid::new_v4(), 1000), 1000, true).unwrap();
    let database = &projection.quality[0].databases[0];
    let mut fields = database.fields.clone();
    fields.push(crate::ip_quality::QualityField {
        label: "干净".into(),
        value: json!(true),
        kind: None,
    });
    fields.push(crate::ip_quality::QualityField {
        label: "代理".into(),
        value: json!("false"),
        kind: None,
    });
    let validated = validate_cached_fields(&database.database, fields);
    assert_eq!(validated.len(), database.fields.len());
    assert!(validated.iter().all(|field| field.kind.is_some()));
}

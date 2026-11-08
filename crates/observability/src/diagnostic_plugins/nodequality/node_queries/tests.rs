use super::*;
use serde_json::json;
use uuid::Uuid;

fn receipt() -> (Value, DiagnosticSectionUpdate, i64) {
    let id = Uuid::new_v4();
    let now = now_timestamp();
    let job = json!({"id":id,"plugin":"nodequality","version":NODE_QUERY_VERSION,
        "options":{"mode":"ip","ip_version":"both","node_ips":"[\"1.1.1.1\"]"}});
    let rows = [
        json!({"provider":"ipregistry-node","database":"ipregistry-v1","target_ip":"1.1.1.1",
            "execution":"node","source":"https://api.ipregistry.co","available":true,"observed_ip":"1.1.1.1",
            "attempted_at":now,"elapsed_ms":0,"data":{"ip":"1.1.1.1","type":"IPv4","security":{"is_proxy":false}},"error":null}),
        json!({"provider":"dbip-node","database":"dbip-v2","target_ip":"1.1.1.1",
            "execution":"node","source":"https://api.db-ip.com/v2","available":true,"observed_ip":"1.1.1.1",
            "attempted_at":now,"elapsed_ms":0,"data":{"ipAddress":"1.1.1.1","latitude":0,"isProxy":false},"error":null}),
    ];
    let report = json!({"schema":SCHEMA,"job_id":id,"execution":"node","ip_version":"both",
        "started_at":now,"finished_at":now,"ips":["1.1.1.1"],"results":rows,
        "streaming":{"execution":"node","status":"unknown","reason":"TEST_ONLY 未配置正式流媒体授权，信息未知"}});
    (
        job,
        DiagnosticSectionUpdate {
            id,
            name: "ip_quality".into(),
            text: report.to_string(),
            complete: true,
            revision: 1,
            collected_at: now,
        },
        now,
    )
}

fn alter(update: &mut DiagnosticSectionUpdate, pointer: &str, value: Value) {
    let mut report: Value = serde_json::from_str(&update.text).unwrap();
    *report.pointer_mut(pointer).unwrap() = value;
    update.text = report.to_string();
}

#[test]
fn official_node_receipt_preserves_false_zero_and_source_identity() {
    let (job, update, now) = receipt();
    let parsed = parse_section(&job, &update, now, now + 90)
        .unwrap()
        .unwrap()
        .quality;
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].databases[0].fields[0].value, json!(false));
    assert_eq!(
        parsed[1].databases[0]
            .fields
            .iter()
            .find(|field| field.label == "纬度")
            .unwrap()
            .value,
        json!(0)
    );
    for result in parsed {
        assert_eq!(result.status, "succeeded");
        assert_eq!(result.last_success_at, Some(now));
        assert_eq!(result.databases[0].execution, "node");
        assert_eq!(result.databases[0].observed_ip.as_deref(), Some("1.1.1.1"));
        assert!(
            !result.databases[0]
                .source
                .as_ref()
                .unwrap()
                .contains("TEST_ONLY_private")
        );
    }
}

#[test]
fn mixed_wrong_types_fail_the_source_without_losing_other_source() {
    for wrong in [json!(0), json!("false")] {
        let (job, mut update, now) = receipt();
        alter(&mut update, "/results/1/data/isProxy", wrong);
        let parsed = parse_section(&job, &update, now, now + 90)
            .unwrap()
            .unwrap()
            .quality;
        assert_eq!(parsed[0].status, "succeeded");
        assert_eq!(parsed[1].status, "failed");
        assert!(parsed[1].databases[0].fields.is_empty());
        assert_eq!(
            parsed[1].databases[0].error_kind,
            Some(ip_quality::QueryErrorKind::SchemaMismatch)
        );
    }
}

#[test]
fn frozen_identity_time_schema_source_and_secret_keys_are_enforced() {
    for (pointer, wrong) in [
        ("/schema", json!("opaque-json")),
        ("/job_id", json!(Uuid::new_v4())),
        ("/execution", json!("panel")),
        ("/streaming/status", json!("unlocked")),
        (
            "/results/0/source",
            json!("https://api.db-ip.com/v2/TEST_ONLY_private"),
        ),
        ("/results/0/provider", json!("check-place")),
        ("/results/0/database", json!("dbip-v2")),
        ("/results/0/observed_ip", json!("8.8.8.8")),
        ("/results/0/target_ip", json!("8.8.8.8")),
        ("/results/0/data/ip", json!("8.8.8.8")),
        ("/results/0/elapsed_ms", json!(75001)),
        (
            "/results/0/data",
            json!({"ip":"1.1.1.1","type":"IPv4","api_key":"TEST_ONLY_private"}),
        ),
    ] {
        let (job, mut update, now) = receipt();
        alter(&mut update, pointer, wrong);
        assert!(
            parse_section(&job, &update, now, now + 90).is_err(),
            "{pointer}"
        );
    }
    let (mut job, update, now) = receipt();
    job["version"] = json!(PLUGIN_VERSION);
    assert!(parse_section(&job, &update, now, now + 90).is_err());
}

#[test]
fn per_source_unknown_and_http_errors_preserve_attempt_semantics() {
    for (kind, status, attempted) in [
        ("not_attempted", Value::Null, false),
        ("http_403", json!(403), true),
        ("http_429", json!(429), true),
        ("http_other", json!(401), true),
        ("timeout", Value::Null, true),
    ] {
        let (job, mut update, now) = receipt();
        alter(&mut update, "/results/1/data", Value::Null);
        alter(
            &mut update,
            "/results/1/error",
            json!({"kind":kind,"message":"TEST_ONLY refused private key must not be echoed","http_status":status}),
        );
        if !attempted {
            alter(&mut update, "/results/1/available", json!(false));
            alter(&mut update, "/results/1/attempted_at", Value::Null);
            alter(&mut update, "/results/1/elapsed_ms", Value::Null);
            alter(&mut update, "/results/1/observed_ip", Value::Null);
        }
        let parsed = parse_section(&job, &update, now, now + 90)
            .unwrap()
            .unwrap()
            .quality;
        assert_eq!(parsed[1].status, "failed");
        assert!(parsed[1].databases[0].fields.is_empty());
        assert_eq!(parsed[1].last_success_at, None);
        assert_eq!(parsed[1].last_attempt_at.is_some(), attempted);
        assert!(
            !parsed[1].databases[0]
                .error
                .as_ref()
                .unwrap()
                .contains("private key")
        );
    }
}

#[test]
fn dispatch_requires_exact_version_mode_and_every_capability() {
    let (job, _, _) = receipt();
    let required = json!(NodeIpQualityPlugin.required_capabilities());
    assert!(NodeIpQualityPlugin.can_dispatch(&job, &required));
    assert!(!NodeIpQualityPlugin.can_dispatch(&job, &json!(["diagnostic:nodequality"])));
    let mut wrong = job;
    wrong["version"] = json!(PLUGIN_VERSION);
    assert!(!NodeIpQualityPlugin.can_dispatch(&wrong, &required));
}

#[test]
fn special_use_addresses_cannot_be_reported_as_official_node_sources() {
    for ip in [
        "2001:2::1",
        "3fff::1",
        "3fff:fff:ffff:ffff:ffff:ffff:ffff:ffff",
        "192.88.99.1",
    ] {
        let (mut job, mut update, now) = receipt();
        job["options"]["node_ips"] = json!(serde_json::to_string(&vec![ip]).unwrap());
        let mut report: Value = serde_json::from_str(&update.text).unwrap();
        report["ips"] = json!([ip]);
        for row in report["results"].as_array_mut().unwrap() {
            row["target_ip"] = json!(ip);
            row["observed_ip"] = json!(ip);
            if row["provider"] == "ipregistry-node" {
                row["data"]["ip"] = json!(ip);
                row["data"]["type"] = json!(if ip.contains(':') { "IPv6" } else { "IPv4" });
            } else {
                row["data"]["ipAddress"] = json!(ip);
            }
        }
        update.text = report.to_string();
        assert!(parse_section(&job, &update, now, now + 90).is_err(), "{ip}");
    }
}

#[test]
fn only_exact_registered_r21_ip_jobs_bypass_the_full_start_gate() {
    let (job, _, _) = receipt();
    assert!(
        crate::diagnostic_plugins::for_job(&job)
            .unwrap()
            .start_denial(&job)
            .is_none()
    );
    for version in [
        PLUGIN_VERSION,
        "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r20",
        "unknown",
    ] {
        let mut invalid = job.clone();
        invalid["version"] = json!(version);
        assert!(
            crate::diagnostic_plugins::for_job(&invalid)
                .unwrap()
                .start_denial(&invalid)
                .is_some()
        );
    }
    for mode in [Value::Null, json!("full")] {
        let mut full = job.clone();
        full["options"]["mode"] = mode;
        assert!(
            crate::diagnostic_plugins::for_job(&full)
                .unwrap()
                .start_denial(&full)
                .is_some()
        );
    }
}

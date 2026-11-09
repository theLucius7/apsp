use super::*;

fn node(credential: &str) -> apsp_compiler::Node {
    serde_json::from_value(json!({"id":1,"name":"Example node","port":443,"public_host":"proxy.example.invalid","sni":"proxy.example.invalid","private_key":"TEST_ONLY-private-key","public_key":"TEST_ONLY-public-key","short_id":"01020304","users":[{"user_id":1,"uuid":"00000000-0000-4000-8000-000000000001","credential":credential}]})).expect("node fixture")
}

#[test]
fn credential_only_change_is_visible_without_exposing_secrets() {
    let changes =
        differences(&[node("TEST_ONLY-old")], &[node("TEST_ONLY-new")]).expect("comparison");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["change"], "changed");
    assert_eq!(changes[0]["affected_users"], json!([1]));
    let serialized = serde_json::to_string(&changes).expect("output");
    for secret in ["TEST_ONLY-old", "TEST_ONLY-new", "TEST_ONLY-private-key"] {
        assert!(!serialized.contains(secret));
    }
}

#[test]
fn checkpoint_failure_uses_evidence_without_exposing_raw_runtime_output() {
    assert_eq!(
        inspection_reason("configuration file differs from the deployment").0,
        "configuration_mismatch"
    );
    assert_eq!(
        inspection_reason("controlled instance is executing another binary").0,
        "runtime_identity_mismatch"
    );
    assert_eq!(inspection_reason("runtime is not healthy").0, "unhealthy");
    assert_eq!(
        inspection_reason("TEST_ONLY-secret-bearing-unclassified-error").0,
        "inspection_failed"
    );
    assert!(
        !inspection_reason("TEST_ONLY-secret-bearing-unclassified-error")
            .1
            .contains("TEST_ONLY")
    );
}

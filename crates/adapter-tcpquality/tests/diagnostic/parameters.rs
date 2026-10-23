use super::*;

#[tokio::test]
async fn invalid_whitelist_digest_identity_paths_and_targets_are_rejected_before_calls() {
    let fixture = Fixture::new(1);
    let adapter = TcpQualityAdapter::new();
    for (key, value) in [
        ("ip_version", "both"),
        ("count", "16"),
        ("count", "04"),
        ("concurrency", "0"),
        ("upload_report", "false"),
        ("command", "id"),
        ("region", "untrusted"),
        ("environment_section", "unknown"),
        ("url", "https://example.test"),
        ("no_rootfs", "true"),
        ("allow_speedtest_staged", "false"),
        (
            "target_digest",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ),
    ] {
        let mut spec = fixture.spec();
        spec.options.insert(key.into(), value.into());
        let privileged = Recorder::default();
        assert!(adapter.prepare(&spec, &privileged).await.is_err(), "{key}");
        assert!(privileged.probes.lock().unwrap().is_empty(), "{key}");
        assert!(privileged.created.lock().unwrap().is_empty(), "{key}");
    }
    let mut cases = Vec::new();
    let mut wrong = fixture.spec();
    wrong.version = "0.3.0-main-r1".into();
    cases.push(wrong);
    let mut wrong = fixture.spec();
    wrong.version = format!("0.3.0-{}-r1", "A".repeat(40));
    cases.push(wrong);
    let mut wrong = fixture.spec();
    wrong.id = "$(id)".into();
    cases.push(wrong);
    let mut wrong = fixture.spec();
    wrong.job_dir = fixture.root.join("../escape");
    cases.push(wrong);
    let mut wrong = fixture.spec();
    wrong.job_dir = fixture.root.join("other");
    cases.push(wrong);
    let mut wrong = fixture.spec();
    wrong.binary_path = PathBuf::from("/tmp/$program");
    cases.push(wrong);
    let mut wrong = fixture.spec();
    wrong.timeout_secs = 0;
    cases.push(wrong);
    let mut wrong = fixture.spec();
    wrong.options.remove("ip_version");
    cases.push(wrong);
    let mut wrong = fixture.spec();
    wrong.options.remove("targets");
    cases.push(wrong);
    for spec in cases {
        let privileged = Recorder::default();
        assert!(adapter.prepare(&spec, &privileged).await.is_err());
        assert!(privileged.probes.lock().unwrap().is_empty());
    }
    for scope in [
        json!({"schema":1,"targets":[]}),
        json!({"schema":1,"targets":[fixture.scope["targets"][0].clone(),fixture.scope["targets"][0].clone()]}),
        json!({"schema":1,"targets":[{"id":"10000000-0000-4000-8000-000000000000","name":"bad","target":"https://example.test","port":443,"carrier":"configured","region":null}]}),
        json!({"schema":1,"targets":fixture.scope["targets"],"command":"unexpected"}),
        json!({"schema":1,"targets":(0..9).map(|_|fixture.scope["targets"][0].clone()).collect::<Vec<_>>()}),
    ] {
        let mut spec = fixture.spec();
        let bytes = scope.to_string();
        spec.options.insert(
            "target_digest".into(),
            format!("{:x}", Sha256::digest(bytes.as_bytes())),
        );
        spec.options.insert("targets".into(), bytes);
        let privileged = Recorder::default();
        assert!(adapter.prepare(&spec, &privileged).await.is_err());
        assert!(privileged.probes.lock().unwrap().is_empty());
    }
    let mut large = fixture.spec();
    large
        .options
        .insert("targets".into(), "x".repeat(16 * 1024 + 1));
    assert!(adapter.prepare(&large, &Recorder::default()).await.is_err());
}

#[tokio::test]
async fn wrong_build_identity_null_pin_extra_fields_and_truncation_never_prepare_job() {
    let fixture = Fixture::new(1);
    let adapter = TcpQualityAdapter::new();
    for bad in [
        json!({"version":"0.3.0","source_repo":"theLucius7/apsp","source_commit":null}),
        json!({"version":"0.3.0","source_repo":"other/example","source_commit":fixture::SOURCE}),
        json!({"version":"0.4.0","source_repo":"theLucius7/apsp","source_commit":fixture::SOURCE}),
        json!({"version":"0.3.0","source_repo":"theLucius7/apsp","source_commit":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}),
        json!({"version":"0.3.0","source_repo":"theLucius7/apsp","source_commit":fixture::SOURCE,"upload":true}),
    ] {
        let privileged = Recorder {
            bad: Some(bad),
            ..Default::default()
        };
        assert!(adapter.prepare(&fixture.spec(), &privileged).await.is_err());
        assert_eq!(
            *privileged.probes.lock().unwrap(),
            vec!["--version", "--build-info"]
        );
        assert!(privileged.created.lock().unwrap().is_empty());
        assert!(privileged.written.lock().unwrap().is_empty());
    }
    for privileged in [
        Recorder {
            bad_version: true,
            ..Default::default()
        },
        Recorder {
            truncated: true,
            ..Default::default()
        },
    ] {
        assert!(adapter.prepare(&fixture.spec(), &privileged).await.is_err());
        assert!(privileged.created.lock().unwrap().is_empty());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn prepare_rejects_symlink_binary_and_workspace_before_mutating_outside_scope() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new(1);
    let spec = fixture.spec();
    fixture.workspace();
    std::fs::set_permissions(&spec.job_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        TcpQualityAdapter::new()
            .prepare(&spec, &Recorder::default())
            .await
            .is_err()
    );
    std::fs::remove_dir(&spec.job_dir).unwrap();
    symlink(&fixture.root, &spec.job_dir).unwrap();
    let privileged = Recorder::default();
    assert!(
        TcpQualityAdapter::new()
            .prepare(&spec, &privileged)
            .await
            .is_err()
    );
    assert!(privileged.created.lock().unwrap().is_empty());
    std::fs::remove_file(&spec.job_dir).unwrap();
    let original = spec.binary_path.with_file_name("original");
    std::fs::rename(&spec.binary_path, &original).unwrap();
    symlink(&original, &spec.binary_path).unwrap();
    let privileged = Recorder::default();
    assert!(
        TcpQualityAdapter::new()
            .prepare(&spec, &privileged)
            .await
            .is_err()
    );
    assert!(privileged.probes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn regions_are_frozen_labels_and_ipv6_does_not_allow_arbitrary_region_options() {
    for region in [
        None,
        Some("east_asia"),
        Some("southeast_asia"),
        Some("europe"),
        Some("americas"),
        Some("other"),
    ] {
        let mut fixture = Fixture::new(1);
        fixture.scope["targets"][0]["target"] = json!("::1");
        fixture.scope["targets"][0]["region"] = region.map(Value::from).unwrap_or(Value::Null);
        let mut spec = fixture.spec();
        spec.options.insert("ip_version".into(), "6".into());
        let job = TcpQualityAdapter::new()
            .prepare(&spec, &Recorder::default())
            .await
            .unwrap();
        assert!(
            job.args
                .windows(2)
                .any(|args| args == ["--ip-version", "6"])
        );
        let saved: Value =
            serde_json::from_slice(&std::fs::read(spec.job_dir.join("targets.json")).unwrap())
                .unwrap();
        assert_eq!(
            saved["targets"][0]["region"],
            fixture.scope["targets"][0]["region"]
        );
    }
    let mut fixture = Fixture::new(1);
    for bad in ["configured", "unverified geography"] {
        fixture.scope["targets"][0]["region"] = json!(bad);
        let privileged = Recorder::default();
        assert!(
            TcpQualityAdapter::new()
                .prepare(&fixture.spec(), &privileged)
                .await
                .is_err()
        );
        assert!(privileged.probes.lock().unwrap().is_empty());
    }
    let too_many = Fixture::new(9);
    assert!(
        TcpQualityAdapter::new()
            .prepare(&too_many.spec(), &Recorder::default())
            .await
            .is_err()
    );
}

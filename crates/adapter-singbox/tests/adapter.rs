#![forbid(unsafe_code)]

mod support;

use apsp_adapter_sdk::{Adapter, Plan, Prepared};
use apsp_adapter_singbox::SingboxAdapter;
use support::{TempDir, TestOps, TestServices};

#[tokio::test]
async fn signed_path_plan_requires_matching_staged_bytes_native_features_and_private_controller() {
    let directory = TempDir::new();
    let mut spec = directory.spec(18085, serde_json::json!([]));
    let mut native: serde_json::Value = serde_json::from_str(&spec.files["config.json"]).unwrap();
    native["outbounds"] = serde_json::json!([{"type":"direct","tag":"direct"},{"type":"vless","tag":"chain-1-g2-h1","server":"127.0.0.1","server_port":20000,
        "uuid":"00000000-0000-0000-0000-000000000001"}]);
    native["experimental"]["clash_api"] =
        serde_json::json!({"external_controller":"127.0.0.1:18086","secret":"a".repeat(64)});
    spec.files.insert("config.json".into(), native.to_string());
    let plan = serde_json::json!({"schema":1,"runtime_version":"1.14.2","required_build_tags":["with_clash_api","with_v2ray_api"],
        "bindings":[{"id":"00000000-0000-0000-0000-000000000002","selector":"chain-1-g2-h1","target":"https://panel.example/health"}]}).to_string();
    spec.files
        .insert("runtime-probes.json".into(), plan.clone());
    std::fs::write(directory.0.join("config.json"), &spec.files["config.json"]).unwrap();
    std::fs::write(directory.0.join("runtime-probes.json"), &plan).unwrap();
    let complete = TestOps {
        version: Some("sing-box version 1.14.2\nTags: with_v2ray_api,with_clash_api\n".into()),
        ..TestOps::default()
    };
    assert!(
        SingboxAdapter::new()
            .prepare(spec.clone(), &complete)
            .await
            .is_ok()
    );
    assert_eq!(complete.commands.lock().unwrap().len(), 2);
    let incomplete = TestOps::default();
    assert!(
        SingboxAdapter::new()
            .prepare(spec.clone(), &incomplete)
            .await
            .is_err()
    );
    assert_eq!(incomplete.commands.lock().unwrap().len(), 1);
    std::fs::write(directory.0.join("runtime-probes.json"), "{}").unwrap();
    let untouched = TestOps::default();
    assert!(
        SingboxAdapter::new()
            .prepare(spec, &untouched)
            .await
            .is_err()
    );
    assert!(untouched.commands.lock().unwrap().is_empty());
}

#[tokio::test]
async fn prepares_only_checked_native_configuration() {
    let directory = TempDir::new();
    let spec = directory.spec(
        18085,
        serde_json::json!([{
            "type":"vless", "listen":"::", "listen_port":20000
        }]),
    );
    let ops = TestOps::default();
    let adapter = SingboxAdapter::new();
    let prepared = adapter.prepare(spec.clone(), &ops).await.unwrap();
    assert_eq!(prepared.listen_ports, vec![20000]);
    let commands = ops.commands.lock().unwrap();
    assert_eq!(commands[0], ["version"]);
    assert_eq!(
        commands[1],
        [
            "check",
            "-c",
            spec.revision_dir.join("config.json").to_str().unwrap()
        ]
    );
    assert_eq!(adapter.describe().module, "singbox");
    assert_eq!(adapter.describe().service_group, "apsp-singbox");
    assert!(adapter.usage_source().is_some());
}

#[tokio::test]
async fn rejects_wrong_version_missing_tags_and_failed_native_check() {
    let directory = TempDir::new();
    let spec = directory.spec(18085, serde_json::json!([]));
    for version in [
        "sing-box version 1.14.1\nTags: with_v2ray_api\n",
        "sing-box version 1.14.2\nTags: with_quic\n",
    ] {
        let ops = TestOps {
            version: Some(version.into()),
            ..TestOps::default()
        };
        assert!(
            SingboxAdapter::new()
                .prepare(spec.clone(), &ops)
                .await
                .is_err()
        );
        assert_eq!(ops.commands.lock().unwrap().len(), 1);
    }
    let ops = TestOps {
        check_ok: false,
        ..TestOps::default()
    };
    assert!(
        SingboxAdapter::new()
            .prepare(spec.clone(), &ops)
            .await
            .is_err()
    );
    std::fs::write(spec.revision_dir.join("config.json"), "{}").unwrap();
    assert!(
        SingboxAdapter::new()
            .prepare(spec, &TestOps::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn rejects_exposed_statistics_and_conflicting_native_ports() {
    let directory = TempDir::new();
    let spec = directory.spec(18085, serde_json::json!([]));
    for address in [
        "0.0.0.0:18085",
        "[::1]:18085",
        "localhost:18085",
        "127.0.0.1:0",
    ] {
        let mut target = spec.clone();
        target.stats_listen = address.into();
        assert!(
            SingboxAdapter::new()
                .prepare(target, &TestOps::default())
                .await
                .is_err()
        );
    }
    for inbounds in [
        serde_json::json!([{"type":"vless", "listen":"::", "listen_port":18085}]),
        serde_json::json!([{"type":"vless", "listen":"::", "listen_port":0}]),
        serde_json::json!([{"type":"vless", "listen":"::", "listen_port":65536}]),
        serde_json::json!([{"type":"vless", "listen":"::", "listen_port":20000}, {"type":"vless", "listen":"127.0.0.1", "listen_port":20000}]),
    ] {
        let target = directory.spec(18085, inbounds);
        assert!(
            SingboxAdapter::new()
                .prepare(target, &TestOps::default())
                .await
                .is_err()
        );
    }
    let mut mismatched = directory.spec(18085, serde_json::json!([]));
    mismatched.stats_listen = "127.0.0.1:18086".into();
    assert!(
        SingboxAdapter::new()
            .prepare(mismatched, &TestOps::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn plans_by_kernel_version_before_configuration_hash() {
    let directory = TempDir::new();
    let previous = Prepared {
        spec: directory.spec(18085, serde_json::json!([])),
        listen_ports: vec![],
    };
    let adapter = SingboxAdapter::new();
    assert_eq!(adapter.plan(None, &previous).await.unwrap(), Plan::Restart);
    assert_eq!(
        adapter.plan(Some(&previous), &previous).await.unwrap(),
        Plan::Noop
    );
    let mut next = previous.clone();
    next.spec.revision += 1;
    assert_eq!(
        adapter.plan(Some(&previous), &next).await.unwrap(),
        Plan::Noop
    );
    next.spec.config_hash = "b".repeat(64);
    assert_eq!(
        adapter.plan(Some(&previous), &next).await.unwrap(),
        Plan::Reload
    );
    next.spec.config_hash = previous.spec.config_hash.clone();
    next.spec.kernel_version = "1.14.3".into();
    assert_eq!(
        adapter.plan(Some(&previous), &next).await.unwrap(),
        Plan::Restart
    );
    let services = TestServices::default();
    adapter
        .apply(Plan::Noop, &previous, &services)
        .await
        .unwrap();
    assert!(services.calls.lock().unwrap().is_empty());
    adapter
        .apply(Plan::Restart, &next, &services)
        .await
        .unwrap();
    assert_eq!(*services.calls.lock().unwrap(), ["restart"]);
}

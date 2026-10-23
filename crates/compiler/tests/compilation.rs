#![forbid(unsafe_code)]

use apsp_compiler::{Access, Node, compile_client, compile_server, stat_name, subscription_links};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::{fs, path::PathBuf};
use uuid::Uuid;

fn nodes() -> Vec<Node> {
    vec![
        Node {
            enabled: true,
            settings: Default::default(),
            id: 3,
            name: "测试节点 / 主入口".into(),
            port: 20000,
            public_host: "proxy.example.com".into(),
            sni: "www.example.com".into(),
            // Synthetic fixture bytes, never usable production credentials.
            private_key: "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE".into(),
            public_key: "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI".into(),
            short_id: "1234abcd".into(),
            protocol_config: Default::default(),
            users: vec![
                Access {
                    user_id: 2,
                    uuid: Uuid::from_u128(2),
                    credential: String::new(),
                },
                Access {
                    user_id: 1,
                    uuid: Uuid::from_u128(1),
                    credential: String::new(),
                },
            ],
        },
        Node {
            enabled: true,
            settings: Default::default(),
            id: 8,
            name: "空节点".into(),
            port: 20001,
            public_host: "empty.example.com".into(),
            sni: "www.example.com".into(),
            private_key: "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE".into(),
            public_key: "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI".into(),
            short_id: "abcd1234".into(),
            protocol_config: Default::default(),
            users: vec![],
        },
    ]
}

fn golden(name: &str, value: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
        fs::write(&path, value).unwrap();
    }
    assert_eq!(
        value,
        fs::read_to_string(&path).unwrap(),
        "golden: {}",
        path.display()
    );
}

#[test]
fn service_configuration_matches_golden() {
    let output = compile_server(&nodes()).unwrap();
    golden("server.json", &output);
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["inbounds"].as_array().unwrap().len(), 1);
    assert_eq!(value["inbounds"][0]["tag"], "node-3");
    assert_eq!(value["inbounds"][0]["users"][0]["name"], "u1_n3");
    assert_eq!(value["inbounds"][0]["users"][1]["name"], "u2_n3");
    assert_eq!(
        value["experimental"]["v2ray_api"]["listen"],
        "127.0.0.1:18085"
    );
    assert_eq!(
        value["experimental"]["v2ray_api"]["stats"]["users"],
        serde_json::json!(["u1_n3", "u2_n3"])
    );
}

#[test]
fn empty_model_has_no_exposed_inbounds() {
    golden("empty.json", &compile_server(&[]).unwrap());
    let only_empty = vec![nodes().remove(1)];
    assert_eq!(
        compile_server(&only_empty).unwrap(),
        compile_server(&[]).unwrap()
    );
}

#[test]
fn settings_preserve_old_snapshots_and_separate_public_from_listening_endpoint() {
    let mut model = nodes();
    let old = serde_json::to_value(&model).unwrap();
    assert!(old[0].get("enabled").is_none());
    assert!(old[0].get("settings").is_none());
    assert_eq!(
        compile_server(&model).unwrap(),
        compile_server(&serde_json::from_value::<Vec<Node>>(old).unwrap()).unwrap()
    );
    model[0].settings.listen = "127.0.0.1".into();
    model[0].settings.public_port = Some(443);
    model[0].settings.tcp_fast_open = true;
    model[0].settings.reality.handshake_server = Some("handshake.example.com".into());
    model[0].settings.reality.handshake_port = 8443;
    model[0].settings.reality.fingerprint = apsp_compiler::Fingerprint::Firefox;
    let native: Value = serde_json::from_str(&compile_server(&model).unwrap()).unwrap();
    assert_eq!(native["inbounds"][0]["listen"], "127.0.0.1");
    assert_eq!(native["inbounds"][0]["listen_port"], 20000);
    assert_eq!(native["inbounds"][0]["tcp_fast_open"], true);
    assert_eq!(
        native["inbounds"][0]["tls"]["reality"]["handshake"],
        serde_json::json!({"server":"handshake.example.com","server_port":8443})
    );
    let client: Value = serde_json::from_str(&compile_client(&model, 1).unwrap()).unwrap();
    assert_eq!(client["outbounds"][1]["server_port"], 443);
    assert_eq!(
        client["outbounds"][1]["tls"]["utls"]["fingerprint"],
        "firefox"
    );
    let links = String::from_utf8(
        STANDARD
            .decode(subscription_links(&model, 1).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert!(links.contains("proxy.example.com:443?"));
    assert!(links.contains("fp=firefox"));
    model[0].enabled = false;
    assert_eq!(
        compile_server(&model).unwrap(),
        compile_server(&[]).unwrap()
    );
    assert!(matches!(
        compile_client(&model, 1),
        Err(apsp_compiler::CompileError::NoAuthorizedNodes(1))
    ));
    assert_eq!(subscription_links(&model, 1).unwrap(), "");
    assert_eq!(model[0].users.len(), 2);
}

#[test]
fn input_order_cannot_change_compiled_bytes() {
    let original = nodes();
    let mut reversed = original.clone();
    reversed.reverse();
    for node in &mut reversed {
        node.users.reverse();
    }
    assert_eq!(
        compile_server(&original).unwrap(),
        compile_server(&reversed).unwrap()
    );
    assert_eq!(
        compile_client(&original, 1).unwrap(),
        compile_client(&reversed, 1).unwrap()
    );
    assert_eq!(
        subscription_links(&original, 1).unwrap(),
        subscription_links(&reversed, 1).unwrap()
    );
}

#[test]
fn client_has_only_the_requested_users_credentials() {
    let model = nodes();
    let output = compile_client(&model, 1).unwrap();
    golden("client.json", &output);
    assert!(output.contains(&Uuid::from_u128(1).to_string()));
    assert!(!output.contains(&Uuid::from_u128(2).to_string()));
    assert!(!output.contains("private_key"));
    assert!(!output.contains(&model[0].private_key));
    assert!(!output.contains("empty.example.com"));
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["inbounds"][0]["type"], "mixed");
    assert_eq!(value["inbounds"][0]["listen"], "127.0.0.1");
    assert_eq!(value["inbounds"][0]["listen_port"], 2080);
    let outbounds = value["outbounds"].as_array().unwrap();
    let selector = outbounds
        .iter()
        .find(|item| item["type"] == "selector")
        .unwrap();
    assert_eq!(value["route"]["final"], selector["tag"]);
    let proxy = outbounds
        .iter()
        .find(|item| item["type"] == "vless")
        .unwrap();
    assert_eq!(proxy["tls"]["utls"]["fingerprint"], "chrome");
}

#[test]
fn sharing_links_are_encoded_and_isolate_users() {
    let model = nodes();
    let raw = String::from_utf8(
        STANDARD
            .decode(subscription_links(&model, 1).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(raw.lines().count(), 1);
    assert!(
        raw.starts_with("vless://00000000-0000-0000-0000-000000000001@proxy.example.com:20000?")
    );
    assert!(raw.contains("flow=xtls-rprx-vision"));
    assert!(raw.contains("security=reality"));
    assert!(raw.contains("pbk=AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI"));
    assert!(raw.contains("sid=1234abcd"));
    assert!(raw.contains("%E6%B5%8B%E8%AF%95"));
    assert!(!raw.contains(&Uuid::from_u128(2).to_string()));
    assert!(!raw.contains(&model[0].private_key));
    assert_eq!(subscription_links(&model, 99).unwrap(), "");
}

#[test]
fn ipv6_hosts_have_brackets_in_sharing_links() {
    let mut model = nodes();
    model[0].public_host = "2001:db8::1".into();
    let raw = String::from_utf8(
        STANDARD
            .decode(subscription_links(&model, 1).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert!(raw.contains("@[2001:db8::1]:20000?"));
}

#[test]
fn invalid_or_ambiguous_models_are_rejected() {
    let mut model = nodes();
    model[1].id = model[0].id;
    assert!(compile_server(&model).is_err());
    let mut model = nodes();
    model[1].port = model[0].port;
    assert!(compile_server(&model).is_err());
    let mut model = nodes();
    let duplicate = model[0].users[0].clone();
    model[0].users.push(duplicate);
    assert!(compile_server(&model).is_err());
    for bad in [
        "bad key",
        "",
        "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=",
    ] {
        let mut model = nodes();
        model[0].private_key = bad.into();
        assert!(compile_server(&model).is_err(), "private key: {bad}");
    }
    for bad in [
        "bad.example/path",
        "bad.example\n",
        "",
        "https://www.example.com",
    ] {
        let mut model = nodes();
        model[0].sni = bad.into();
        assert!(compile_server(&model).is_err(), "SNI: {bad:?}");
    }
    let mut model = nodes();
    model[0].short_id = "zzzzzzzz".into();
    assert!(compile_server(&model).is_err());
    assert_eq!(stat_name(1, 3), "u1_n3");
}

#[test]
#[ignore = "requires an upstream v1.14.2 binary with with_v2ray_api; set APSP_TEST_SINGBOX"]
fn upstream_runtime_accepts_compiled_server_and_client() {
    let binary = std::env::var("APSP_TEST_SINGBOX").expect("set APSP_TEST_SINGBOX");
    let directory = std::env::temp_dir().join(format!("apsp-compiler-{}", Uuid::new_v4()));
    fs::create_dir_all(&directory).unwrap();
    for (name, text) in [
        ("server", compile_server(&nodes()).unwrap()),
        ("client", compile_client(&nodes(), 1).unwrap()),
    ] {
        let path = directory.join(format!("{name}.json"));
        fs::write(&path, text).unwrap();
        let result = std::process::Command::new(&binary)
            .args(["check", "-c"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

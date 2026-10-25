#![forbid(unsafe_code)]
use apsp_compiler::{
    Node, ProtocolConfig, SsMethod, compile_server,
    preserved::{
        PreservedNode, SESSION_CONTROL_LISTEN, SessionControl, compile_preserved_server,
        compile_preserved_server_with, nexus_access,
    },
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use uuid::Uuid;

fn fixture(id: i64) -> PreservedNode {
    let protocol = ProtocolConfig::Shadowsocks2022 {
        method: SsMethod::Aes128,
        password: STANDARD.encode(b"0123456789abcdef"),
    };
    let node = Node {
        id,
        name: "TEST_ONLY preserved".into(),
        port: 20000 + id as u16,
        public_host: "proxy.example.invalid".into(),
        sni: String::new(),
        private_key: String::new(),
        public_key: String::new(),
        short_id: String::new(),
        enabled: true,
        settings: Default::default(),
        users: vec![nexus_access(
            7,
            Uuid::parse_str("12345678-1234-4234-8234-123456789abc").unwrap(),
            &protocol,
        )],
        protocol_config: protocol,
    };
    let mut configuration: Value =
        serde_json::from_str(&compile_server(std::slice::from_ref(&node)).unwrap()).unwrap();
    configuration
        .as_object_mut()
        .unwrap()
        .remove("experimental");
    configuration["outbounds"] = json!([{"tag":"direct","type":"direct"},{"tag":"block","type":"block"},{"tag":"exit","type":"shadowsocks","server":"egress.example.invalid","server_port":443,"method":"2022-blake3-aes-128-gcm","password":"TEST_ONLY upstream identity","detour":"direct"}]);
    configuration["route"] = json!({"final":"exit","rules":[{"domain_suffix":["blocked.example.invalid"],"outbound":"block"},{"domain":["direct.example.invalid"],"outbound":"direct","invert":true}]});
    PreservedNode {
        node,
        configuration,
    }
}

#[test]
fn ss_rewrite_preserves_wire_password_and_isolates_routes() {
    let first = fixture(1);
    let second = fixture(2);
    assert_eq!(
        first.node.users[0].credential,
        STANDARD.encode(b"12345678-1234-42")
    );
    let text = compile_preserved_server(&[first.clone(), second.clone()]).unwrap();
    assert_eq!(
        text,
        compile_preserved_server(&[second, first.clone()]).unwrap()
    );
    let result: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        result["inbounds"][0]["password"],
        first.configuration["inbounds"][0]["password"]
    );
    assert_eq!(
        result["inbounds"][0]["users"][0]["password"],
        first.node.users[0].credential
    );
    assert_eq!(result["inbounds"][0]["users"][0]["name"], "u7_n1");
    assert_eq!(result["outbounds"][1]["detour"], "n1-direct");
    assert_eq!(result["route"]["rules"][0]["action"], "reject");
    assert_eq!(result["route"]["rules"][1]["rules"][1]["invert"], true);
    assert_eq!(result["route"]["rules"][2]["outbound"], "n1-exit");
    assert_eq!(result["route"]["rules"][5]["outbound"], "n2-exit");
    assert_eq!(
        result["route"]["rules"][3]["rules"][0]["inbound"],
        json!(["node-2"])
    );
}

#[test]
fn refuses_identity_changes_missing_exits_and_unresolved_file_dependencies() {
    let original = fixture(1);
    for mutation in ["password", "port", "files", "route", "dns"] {
        let mut changed = original.clone();
        match mutation {
            "password" => {
                changed.configuration["inbounds"][0]["password"] = json!(STANDARD.encode([9; 16]))
            }
            "port" => changed.configuration["inbounds"][0]["listen_port"] = json!(12345),
            "files" => {
                changed.configuration["inbounds"][0]["tls"] =
                    json!({"certificate_path":"/old/private.pem"})
            }
            "route" => changed.configuration["route"]["final"] = json!("missing"),
            _ => {
                changed.configuration["experimental"] =
                    json!({"clash_api":{"external_controller":"0.0.0.0:9090"}})
            }
        }
        assert!(compile_preserved_server(&[changed]).is_err(), "{mutation}");
    }
    let mut empty = original;
    empty.node.users.clear();
    let config: Value = serde_json::from_str(&compile_preserved_server(&[empty]).unwrap()).unwrap();
    assert_eq!(config["inbounds"], json!([]));
}

#[test]
fn old_reality_short_ids_and_hysteria_uuid_are_accepted_without_rotation() {
    let mut node = fixture(1).node;
    node.protocol_config = ProtocolConfig::VlessReality;
    node.sni = "handshake.example.invalid".into();
    node.private_key = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE".into();
    node.public_key = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI".into();
    for short in ["aa", "aabbccddeeff", "0011223344556677"] {
        node.short_id = short.into();
        assert!(compile_server(&[node.clone()]).is_ok());
    }
    for short in ["a", "abcdefg0", "001122334455667788"] {
        node.short_id = short.into();
        assert!(compile_server(&[node.clone()]).is_err());
    }
    let protocol = ProtocolConfig::Hysteria2 {
        tls: apsp_compiler::TlsConfig::Manual {
            certificate: "-----BEGIN CERTIFICATE-----\nTEST_ONLY\n-----END CERTIFICATE-----".into(),
            key: "-----BEGIN PRIVATE KEY-----\nTEST_ONLY\n-----END PRIVATE KEY-----".into(),
        },
    };
    let identity = Uuid::from_u128(7);
    let access = nexus_access(7, identity, &protocol);
    assert_eq!(access.credential, identity.to_string());
    node.protocol_config = protocol;
    node.users = vec![access];
    assert!(compile_server(&[node]).is_ok());
}

#[test]
fn preserved_vless_flow_must_match_the_model_that_regenerates_users() {
    let mut node = fixture(1).node;
    node.protocol_config = ProtocolConfig::VlessReality;
    node.sni = "handshake.example.invalid".into();
    node.private_key = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE".into();
    node.public_key = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI".into();
    node.short_id = "aabbccddeeff".into();
    node.users = vec![nexus_access(7, Uuid::from_u128(7), &node.protocol_config)];
    let mut configuration: Value =
        serde_json::from_str(&compile_server(std::slice::from_ref(&node)).unwrap()).unwrap();
    configuration
        .as_object_mut()
        .unwrap()
        .remove("experimental");
    configuration["outbounds"] = json!([{"tag":"direct","type":"direct"}]);
    configuration["route"] = json!({"final":"direct"});
    assert_eq!(
        configuration["inbounds"][0]["users"][0]["flow"],
        "xtls-rprx-vision"
    );
    let preserved = PreservedNode {
        node: node.clone(),
        configuration: configuration.clone(),
    };
    assert!(compile_preserved_server(std::slice::from_ref(&preserved)).is_ok());
    // A legacy listener without Vision must not be published with Vision users.
    let mut plain = preserved;
    plain.configuration["inbounds"][0]["users"][0]
        .as_object_mut()
        .unwrap()
        .remove("flow");
    assert!(compile_preserved_server(std::slice::from_ref(&plain)).is_err());
    plain.node.settings.reality.flow = apsp_compiler::RealityFlow::None;
    let text = compile_preserved_server(&[plain]).unwrap();
    let result: Value = serde_json::from_str(&text).unwrap();
    assert!(result["inbounds"][0]["users"][0].get("flow").is_none());
}

/// The snapshot `tools/nexus_runtime_snapshot.py` builds from the legacy
/// builder's own TEST_ONLY output must compile without rotating any identity.
#[test]
fn legacy_builder_snapshot_compiles_with_original_identities() {
    let text = include_str!("../../../tools/nexus-legacy-export/fixture-snapshot.json");
    let snapshot: Value = serde_json::from_str(text).unwrap();
    let identity = Uuid::parse_str("12345678-1234-4234-8234-123456789abc").unwrap();
    let mut inputs = Vec::new();
    for entry in snapshot["nodes"].as_array().unwrap() {
        let mut preserved: PreservedNode =
            serde_json::from_value(entry["preserved"].clone()).unwrap();
        let access = nexus_access(9, identity, &preserved.node.protocol_config);
        preserved.node.users.push(access);
        inputs.push(preserved);
    }
    let result: Value = serde_json::from_str(&compile_preserved_server(&inputs).unwrap()).unwrap();
    let ss = &result["inbounds"][0];
    assert_eq!(ss["listen_port"], 51002);
    assert_eq!(ss["password"], STANDARD.encode(b"0123456789abcdef"));
    assert_eq!(
        ss["users"],
        json!([{"name":"u9_n43","password":STANDARD.encode(b"12345678-1234-42")}])
    );
    let vless = &result["inbounds"][1];
    assert_eq!(
        vless["users"],
        json!([{"name":"u9_n44","uuid":identity,"flow":"xtls-rprx-vision"}])
    );
    assert_eq!(vless["tls"]["reality"]["short_id"], json!(["aabbccddeeff"]));
    assert_eq!(vless["multiplex"], json!({"enabled":true,"padding":true}));
    let outbounds: Vec<_> = result["outbounds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|outbound| outbound["tag"].as_str().unwrap())
        .collect();
    assert_eq!(outbounds, ["n43-direct", "n44-direct", "n44-landing"]);
    let rules = result["route"]["rules"].as_array().unwrap();
    assert!(
        rules
            .iter()
            .all(|rule| rule.get("outbound") != Some(&json!("block")))
    );
    assert!(rules.iter().any(|rule| rule["outbound"] == "n44-landing"
        && rule["rules"][1]["domain_suffix"] == json!(["landing.example.invalid"])));
}

#[test]
fn identical_host_route_options_are_kept_and_differences_refused() {
    let mut first = fixture(1);
    let mut second = fixture(2);
    for input in [&mut first, &mut second] {
        input.configuration["route"]["default_interface"] = json!("eth0");
        input.configuration["route"]["default_domain_resolver"] = json!("local");
        input.configuration["dns"] = json!({"servers":[{"type":"local","tag":"local"}]});
    }
    let result: Value =
        serde_json::from_str(&compile_preserved_server(&[first.clone(), second.clone()]).unwrap())
            .unwrap();
    assert_eq!(result["route"]["default_interface"], "eth0");
    assert_eq!(result["route"]["default_domain_resolver"], "local");
    assert!(result["route"].get("final").is_none());
    second.configuration["route"]
        .as_object_mut()
        .unwrap()
        .remove("default_interface");
    assert!(compile_preserved_server(&[first.clone(), second]).is_err());
    first.configuration["route"]["domain_strategy"] = json!("ipv4_only");
    assert!(compile_preserved_server(&[first]).is_err());
}

#[test]
fn session_control_adds_session_logs_and_a_loopback_controller_only_when_requested() {
    let input = fixture(1);
    let plain: Value =
        serde_json::from_str(&compile_preserved_server(std::slice::from_ref(&input)).unwrap())
            .unwrap();
    assert_eq!(plain["log"]["level"], "warn");
    assert!(plain["experimental"].get("clash_api").is_none());
    let control = SessionControl {
        secret: "ab".repeat(32),
    };
    let text = compile_preserved_server_with(std::slice::from_ref(&input), Some(&control)).unwrap();
    let controlled: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(controlled["log"], json!({"level":"info","timestamp":true}));
    assert_eq!(
        controlled["experimental"]["clash_api"],
        json!({"external_controller":SESSION_CONTROL_LISTEN,"secret":control.secret})
    );
    assert!(
        SESSION_CONTROL_LISTEN.starts_with("127.0.0.1:"),
        "the controller is never published beyond the device"
    );
    // Identities, listeners and routes are unchanged; only logging and control differ.
    for key in ["inbounds", "outbounds", "route"] {
        assert_eq!(plain[key], controlled[key], "{key}");
    }
    assert_eq!(
        plain["experimental"]["v2ray_api"],
        controlled["experimental"]["v2ray_api"]
    );
    for secret in ["short".to_string(), "zz".repeat(32), "a".repeat(129)] {
        assert!(
            compile_preserved_server_with(
                std::slice::from_ref(&input),
                Some(&SessionControl { secret })
            )
            .is_err()
        );
    }
}

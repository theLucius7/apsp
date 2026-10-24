#![forbid(unsafe_code)]
use apsp_compiler::{
    Node, ProtocolConfig, SsMethod, compile_server,
    preserved::{PreservedNode, compile_preserved_server, nexus_access},
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

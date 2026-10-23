#![forbid(unsafe_code)]
use apsp_compiler::{
    Access, Node,
    client::{ExternalClientNode, compile_with_external},
    compile_client,
    external::ExternalOutbound,
};
use serde_json::{Value, json};
use uuid::Uuid;

fn managed() -> Node {
    Node {
        id: 1,
        name: "受管".into(),
        port: 443,
        public_host: "managed.example.com".into(),
        sni: "www.example.com".into(),
        private_key: "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE".into(),
        public_key: "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI".into(),
        short_id: "1234abcd".into(),
        enabled: true,
        settings: Default::default(),
        protocol_config: Default::default(),
        users: vec![
            Access {
                user_id: 1,
                uuid: Uuid::from_u128(1),
                credential: String::new(),
            },
            Access {
                user_id: 2,
                uuid: Uuid::from_u128(2),
                credential: String::new(),
            },
        ],
    }
}
fn external(id: i64, sort_order: i64) -> ExternalClientNode {
    ExternalClientNode {
        id,
        name: "外部".into(),
        sort_order,
        outbound: ExternalOutbound(
            json!({"type":"shadowsocks","server":"provider.example.com","server_port":443,"method":"aes-128-gcm","password":"TEST_ONLY_provider_credential"}),
        ),
    }
}

#[test]
fn no_external_preserves_existing_client_bytes() {
    let nodes = vec![managed()];
    assert_eq!(
        compile_with_external(&nodes, 1, &[]).unwrap(),
        compile_client(&nodes, 1).unwrap()
    );
}

#[test]
fn mixed_selector_uses_disjoint_stable_ids_and_only_the_requested_managed_user() {
    let nodes = vec![managed()];
    let first = compile_with_external(&nodes, 1, &[external(2, 20), external(1, 10)]).unwrap();
    assert_eq!(
        first,
        compile_with_external(&nodes, 1, &[external(1, 10), external(2, 20)]).unwrap()
    );
    let value: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(
        value["outbounds"][0]["outbounds"],
        json!(["node-1", "external-node-1 外部", "external-node-2 外部"])
    );
    assert_eq!(
        value["outbounds"][2]["password"],
        "TEST_ONLY_provider_credential"
    );
    assert!(!first.contains(&Uuid::from_u128(2).to_string()));
    assert!(!first.contains("private_key"));
    assert!(!first.contains("detour"));
}

#[test]
fn external_names_are_escaped_bounded_and_disambiguated_by_identity() {
    let name = "香港 \\\"线路\\\" \\ A";
    let mut first = external(1, 20);
    let mut second = external(2, 10);
    first.name = name.into();
    second.name = name.into();
    let value: Value =
        serde_json::from_str(&compile_with_external(&[], 1, &[first.clone(), second]).unwrap())
            .unwrap();
    assert_eq!(
        value["outbounds"][0]["outbounds"],
        json!([
            format!("external-node-2 {name}"),
            format!("external-node-1 {name}")
        ])
    );
    assert_eq!(
        value["outbounds"][1]["tag"],
        format!("external-node-2 {name}")
    );
    first.name = "界".repeat(160);
    assert!(compile_with_external(&[], 1, &[first.clone()]).is_ok());
    for name in [String::new(), " ".into(), "换\n行".into(), "界".repeat(161)] {
        first.name = name;
        assert!(compile_with_external(&[], 1, &[first.clone()]).is_err());
    }
}

#[test]
fn external_only_needs_no_managed_access_and_rejects_injected_routes_or_duplicate_identity() {
    let value: Value =
        serde_json::from_str(&compile_with_external(&[], 7, &[external(1, 0)]).unwrap()).unwrap();
    assert_eq!(value["outbounds"].as_array().unwrap().len(), 3);
    assert!(compile_with_external(&[], 0, &[external(1, 0)]).is_err());
    assert!(compile_with_external(&[], 1, &[external(1, 0), external(1, 0)]).is_err());
    let mut injected = external(1, 0);
    injected.outbound.0["detour"] = json!("direct");
    assert!(compile_with_external(&[], 1, &[injected]).is_err());
}

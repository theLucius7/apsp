#![forbid(unsafe_code)]

use apsp_compiler::{
    Access, Node, ProtocolConfig, Relay, compile_client, compile_server_with_relays,
    external::ExternalOutbound,
    paths::{self, Control, Hop, Networks, Path},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;

fn node(id: i64, users: Vec<Access>) -> Node {
    Node {
        id,
        name: format!("fixture-{id}"),
        port: (10440 + id) as u16,
        public_host: format!("node-{id}.example.com"),
        sni: "www.example.com".into(),
        private_key: URL_SAFE_NO_PAD.encode([1; 32]),
        public_key: URL_SAFE_NO_PAD.encode([2; 32]),
        short_id: "1234abcd".into(),
        users,
        enabled: true,
        settings: Default::default(),
        protocol_config: ProtocolConfig::VlessReality,
    }
}

fn user(id: i64) -> Access {
    Access {
        user_id: id,
        uuid: Uuid::from_u128(id as u128),
        credential: String::new(),
    }
}
fn external(id: i64, protocol: &str) -> Hop {
    let mut value =
        json!({"type":protocol,"server":format!("external-{id}.example.com"),"server_port":443});
    match protocol {
        "shadowsocks" => {
            value["method"] = json!("aes-128-gcm");
            value["password"] = json!("external-fixture-secret");
        }
        "socks" => {
            value["version"] = json!("5");
        }
        "vmess" => {
            value["uuid"] = json!(Uuid::from_u128(id as u128 + 100));
            value["security"] = json!("auto");
        }
        "vless" => {
            value["uuid"] = json!(Uuid::from_u128(id as u128 + 100));
        }
        "hysteria2" | "anytls" | "trojan" => {
            value["password"] = json!("external-fixture-secret");
            value["tls"] = json!({"enabled":true,"server_name":"tls.example.com"});
        }
        "tuic" => {
            value["uuid"] = json!(Uuid::from_u128(id as u128 + 100));
            value["password"] = json!("external-fixture-secret");
            value["tls"] = json!({"enabled":true,"server_name":"tls.example.com"});
        }
        "http" => {}
        _ => panic!("fixture protocol"),
    }
    Hop::External {
        node_id: id,
        version_id: id * 10,
        outbound: ExternalOutbound(value),
    }
}
fn managed(id: i64) -> Hop {
    Hop::Managed {
        server_id: id,
        endpoint: Box::new(node(id, vec![])),
        identity: Uuid::from_u128(1000 + id as u128),
    }
}
fn path(hops: Vec<Hop>) -> Path {
    Path {
        chain_id: 1,
        generation: 1,
        entry_server_id: 1,
        entry_node_id: 1,
        active: true,
        hops,
    }
}
fn control() -> Control {
    Control {
        secret: "fixture-control-secret-01234567890123456789".into(),
        test_url: "https://probe.example.com/ready".into(),
    }
}
fn compile(nodes: &[Node], paths: &[Path]) -> paths::Compiled {
    paths::compile(1, nodes, &[], paths, &[], BTreeMap::new(), Some(&control())).unwrap()
}

#[test]
fn lower_transport_is_propagated_backwards_through_real_proxy_semantics() {
    assert_eq!(
        paths::validate(&path(vec![external(10, "http"), managed(2)])).unwrap(),
        Networks {
            tcp: true,
            udp: true
        }
    );
    assert_eq!(
        paths::validate(&path(vec![managed(2), external(10, "http")])).unwrap(),
        Networks {
            tcp: true,
            udp: false
        }
    );
    assert!(paths::validate(&path(vec![external(10, "http"), external(11, "hysteria2")])).is_err());
    assert!(paths::validate(&path(vec![external(10, "http"), external(11, "tuic")])).is_err());
    assert_eq!(
        paths::validate(&path(vec![
            external(10, "http"),
            external(11, "shadowsocks")
        ]))
        .unwrap(),
        Networks {
            tcp: true,
            udp: false
        }
    );
    let mut uot = external(11, "shadowsocks");
    if let Hop::External { outbound, .. } = &mut uot {
        outbound.0["udp_over_tcp"] = json!({"enabled":true,"version":2});
    }
    assert_eq!(
        paths::validate(&path(vec![external(10, "http"), uot])).unwrap(),
        Networks {
            tcp: true,
            udp: true
        }
    );
    assert_eq!(
        paths::validate(&path(vec![external(10, "http"), external(11, "anytls")])).unwrap(),
        Networks {
            tcp: true,
            udp: true
        }
    );
}

#[test]
fn ordered_paths_build_reverse_detours_and_route_only_to_the_last_hop() {
    let entry = node(1, vec![user(7)]);
    for count in 1..=8 {
        let model = path(
            (0..count)
                .map(|position| external(10 + position, "socks"))
                .collect(),
        );
        let compiled = compile(std::slice::from_ref(&entry), std::slice::from_ref(&model));
        let config: Value = serde_json::from_str(&compiled.config).unwrap();
        let outbounds = config["outbounds"].as_array().unwrap();
        for position in 0..count as usize {
            let outbound = outbounds
                .iter()
                .find(|v| v["tag"] == paths::tag(1, 1, position))
                .unwrap();
            if position == 0 {
                assert!(outbound.get("detour").is_none());
            } else {
                assert_eq!(outbound["detour"], paths::tag(1, 1, position - 1));
            }
            assert_eq!(outbound["domain_resolver"], "path-bootstrap");
        }
        let route = config["route"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["action"] == "route" && v["inbound"] == json!(["node-1"]))
            .unwrap();
        assert_eq!(route["outbound"], paths::tag(1, 1, count as usize - 1));
        assert!(!compiled.config.contains("selector") && !compiled.config.contains("urltest"));
        assert_eq!(
            compiled.checks[0].outbound,
            paths::tag(1, 1, count as usize - 1)
        );
    }
    assert!(paths::validate(&path((0..9).map(|id| external(id + 10, "socks")).collect())).is_err());
}

#[test]
fn tcp_only_final_hops_have_an_explicit_udp_rejection_before_routing() {
    let compiled = compile(
        &[node(1, vec![user(7)])],
        &[path(vec![managed(2), external(10, "http")])],
    );
    let config: Value = serde_json::from_str(&compiled.config).unwrap();
    let rules = config["route"]["rules"].as_array().unwrap();
    let reject = rules
        .iter()
        .position(|rule| {
            rule["action"] == "reject"
                && rule["network"] == "udp"
                && rule["inbound"] == json!(["node-1"])
        })
        .expect("UDP rejection");
    let route = rules
        .iter()
        .position(|rule| rule["action"] == "route" && rule["inbound"] == json!(["node-1"]))
        .unwrap();
    assert!(reject < route);
    assert_ne!(rules[route]["outbound"], "direct");
}

#[test]
fn repeated_resources_endpoints_entry_loops_and_unowned_dialers_are_rejected() {
    assert!(paths::validate(&path(vec![managed(1)])).is_err());
    assert!(paths::validate(&path(vec![managed(2), managed(2)])).is_err());
    assert!(paths::validate(&path(vec![external(10, "http"), external(10, "socks")])).is_err());
    let mut duplicate = external(11, "socks");
    if let Hop::External { outbound, .. } = &mut duplicate {
        outbound.0["server"] = json!("external-10.example.com");
    }
    assert!(paths::validate(&path(vec![external(10, "http"), duplicate])).is_err());
    let mut injected = external(10, "socks");
    if let Hop::External { outbound, .. } = &mut injected {
        outbound.0["detour"] = json!("direct");
    }
    assert!(paths::validate(&path(vec![injected])).is_err());
    let entry = node(1, vec![user(7)]);
    let mut returning = external(10, "http");
    if let Hop::External { outbound, .. } = &mut returning {
        outbound.0["server"] = json!(entry.public_host);
        outbound.0["server_port"] = json!(entry.public_port());
    }
    assert!(
        paths::compile(
            1,
            &[entry],
            &[],
            &[path(vec![returning])],
            &[],
            BTreeMap::new(),
            Some(&control())
        )
        .is_err()
    );
}

#[test]
fn only_entry_counts_real_users_and_internal_servers_receive_only_their_identity() {
    let entry = node(1, vec![user(7)]);
    let model = path(vec![managed(2), external(10, "shadowsocks"), managed(3)]);
    let compiled = compile(std::slice::from_ref(&entry), std::slice::from_ref(&model));
    let entry_config: Value = serde_json::from_str(&compiled.config).unwrap();
    assert_eq!(
        entry_config["experimental"]["v2ray_api"]["stats"]["users"],
        json!(["u7_n1"])
    );
    let middle = node(2, vec![user(9)]);
    let compiled = paths::compile(
        2,
        &[middle],
        &[],
        std::slice::from_ref(&model),
        &[],
        BTreeMap::new(),
        None,
    )
    .unwrap();
    let config: Value = serde_json::from_str(&compiled.config).unwrap();
    assert_eq!(
        config["experimental"]["v2ray_api"]["stats"]["users"],
        json!(["u9_n2"])
    );
    assert_eq!(config["inbounds"][0]["users"].as_array().unwrap().len(), 2);
    assert!(!compiled.config.contains("external-fixture-secret"));
    assert!(!compiled.config.contains(&Uuid::from_u128(1003).to_string()));
    let client = compile_client(&[entry], 7).unwrap();
    for secret in [
        "external-fixture-secret",
        "external-10.example.com",
        &Uuid::from_u128(1002).to_string(),
        &Uuid::from_u128(1003).to_string(),
    ] {
        assert!(!client.contains(secret));
    }
}

#[test]
fn candidates_are_checked_without_opening_blocked_public_entries() {
    let mut candidate = path(vec![external(10, "http"), managed(2)]);
    candidate.active = false;
    let compiled = paths::compile(
        1,
        &[node(1, vec![user(7)])],
        &[],
        &[candidate],
        &[1],
        BTreeMap::new(),
        Some(&control()),
    )
    .unwrap();
    let config: Value = serde_json::from_str(&compiled.config).unwrap();
    assert!(config["inbounds"].as_array().unwrap().is_empty());
    assert_eq!(compiled.checks.len(), 1);
    assert!(compiled.constraints.active.is_empty());
    assert!(
        config["experimental"]["v2ray_api"]["stats"]["users"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn input_order_is_deterministic_and_legacy_bytes_remain_unchanged() {
    let first = path(vec![external(10, "http"), managed(2)]);
    let mut second = path(vec![external(11, "http"), managed(2)]);
    second.chain_id = 2;
    second.entry_node_id = 3;
    let entries = vec![node(1, vec![user(7)]), node(3, vec![user(8)])];
    let a = compile(&entries, &[first.clone(), second.clone()]);
    let b = compile(&[entries[1].clone(), entries[0].clone()], &[second, first]);
    assert_eq!(a.config, b.config);
    assert_eq!(
        serde_json::to_value(a.constraints).unwrap(),
        serde_json::to_value(b.constraints).unwrap()
    );
    let relay = Relay {
        settings: Default::default(),
        fingerprint: Default::default(),
        chain_id: 5,
        entry_node_id: 1,
        exit_node_id: 2,
        uuid: Uuid::from_u128(321),
        public_host: "legacy.example.com".into(),
        port: 443,
        sni: "www.example.com".into(),
        public_key: URL_SAFE_NO_PAD.encode([2; 32]),
        short_id: "1234abcd".into(),
    };
    let original = compile_server_with_relays(&entries, std::slice::from_ref(&relay)).unwrap();
    let migrated = paths::compile(1, &entries, &[relay], &[], &[], BTreeMap::new(), None).unwrap();
    assert_eq!(original, migrated.config);
}

#[test]
#[ignore = "requires the official v1.14.2 binary in APSP_TEST_UPSTREAM; native structure checks do not prove a working mixed path"]
fn official_runtime_checks_imported_protocols_and_nested_paths() {
    let binary = std::env::var("APSP_TEST_UPSTREAM").unwrap();
    let directory = std::env::temp_dir().join(format!("apsp-path-check-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let mut models = vec![
        path(vec![external(10, "http"), managed(2)]),
        path(vec![managed(2), external(10, "socks"), managed(3)]),
        path((0..8).map(|id| external(id + 10, "socks")).collect()),
    ];
    for protocol in [
        "shadowsocks",
        "vmess",
        "vless",
        "trojan",
        "hysteria2",
        "tuic",
        "anytls",
        "socks",
        "http",
    ] {
        models.push(path(vec![external(10, protocol)]));
    }
    for (index, model) in models.into_iter().enumerate() {
        let compiled = compile(&[node(1, vec![user(7)])], &[model]);
        let mut config: Value = serde_json::from_str(&compiled.config).unwrap();
        // This official archive omits only the optional accounting extension.
        config["experimental"]
            .as_object_mut()
            .unwrap()
            .remove("v2ray_api");
        let file = directory.join(format!("case-{index}.json"));
        std::fs::write(&file, serde_json::to_vec(&config).unwrap()).unwrap();
        let output = std::process::Command::new(&binary)
            .args(["check", "-c"])
            .arg(file)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "native case {index}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}

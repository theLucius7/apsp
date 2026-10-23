#![forbid(unsafe_code)]

use apsp_compiler::{
    Access, AcmeChallenge, Node, ProtocolConfig, Relay, SsMethod, TlsConfig, compile_client,
    compile_server, compile_server_with_relays,
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde_json::Value;
use uuid::Uuid;

fn node(id: i64, users: Vec<Access>) -> Node {
    Node {
        enabled: true,
        settings: Default::default(),
        id,
        name: format!("node-{id}"),
        port: 443,
        public_host: "proxy.example.com".into(),
        sni: "www.example.com".into(),
        private_key: URL_SAFE_NO_PAD.encode([1; 32]),
        public_key: URL_SAFE_NO_PAD.encode([2; 32]),
        short_id: "1234abcd".into(),
        users,
        protocol_config: ProtocolConfig::VlessReality,
    }
}
fn relay(id: i64) -> Relay {
    Relay {
        settings: Default::default(),
        fingerprint: Default::default(),
        chain_id: id,
        entry_node_id: 1,
        exit_node_id: 2,
        uuid: Uuid::from_u128(11),
        public_host: "exit.example.com".into(),
        port: 443,
        sni: "www.example.com".into(),
        public_key: URL_SAFE_NO_PAD.encode([2; 32]),
        short_id: "1234abcd".into(),
    }
}
#[test]
fn routes_entry_and_counts_only_real_users() {
    let entry = node(
        1,
        vec![Access {
            user_id: 7,
            uuid: Uuid::from_u128(7),
            credential: String::new(),
        }],
    );
    let route = relay(1);
    let config: Value = serde_json::from_str(
        &compile_server_with_relays(std::slice::from_ref(&entry), std::slice::from_ref(&route))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(config["route"]["rules"][0]["outbound"], "chain-1");
    assert_eq!(config["outbounds"][1]["uuid"], route.uuid.to_string());
    assert_eq!(
        config["experimental"]["v2ray_api"]["stats"]["users"],
        serde_json::json!(["u7_n1"])
    );
    let exit: Value = serde_json::from_str(
        &compile_server_with_relays(&[node(2, vec![])], std::slice::from_ref(&route)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        exit["inbounds"][0]["users"][0]["uuid"],
        route.uuid.to_string()
    );
    assert_eq!(
        exit["experimental"]["v2ray_api"]["stats"]["users"],
        serde_json::json!([])
    );
    let client = compile_client(&[entry], 7).unwrap();
    assert!(!client.contains(&route.uuid.to_string()));
    assert!(!client.contains("exit.example.com"));
}
#[test]
fn empty_relays_preserve_existing_bytes_and_invalid_topology_is_rejected() {
    let entry = node(1, vec![]);
    assert_eq!(
        compile_server(std::slice::from_ref(&entry)).unwrap(),
        compile_server_with_relays(std::slice::from_ref(&entry), &[]).unwrap()
    );
    let mut same_host_exit = node(2, vec![]);
    same_host_exit.port = 444;
    assert!(compile_server_with_relays(&[entry.clone(), same_host_exit], &[relay(1)]).is_err());
    assert!(compile_server_with_relays(&[entry], &[relay(1), relay(2)]).is_err());
}
#[test]
fn shared_exit_keeps_direct_stats_and_is_deterministic() {
    let exit = node(
        2,
        vec![Access {
            user_id: 9,
            uuid: Uuid::from_u128(9),
            credential: String::new(),
        }],
    );
    let first = relay(1);
    let mut second = relay(2);
    second.entry_node_id = 3;
    second.uuid = Uuid::from_u128(12);
    let a = compile_server_with_relays(
        std::slice::from_ref(&exit),
        &[first.clone(), second.clone()],
    )
    .unwrap();
    let b = compile_server_with_relays(&[exit], &[second, first]).unwrap();
    assert_eq!(a, b);
    let config: Value = serde_json::from_str(&a).unwrap();
    assert_eq!(config["inbounds"][0]["users"].as_array().unwrap().len(), 3);
    assert_eq!(
        config["experimental"]["v2ray_api"]["stats"]["users"],
        serde_json::json!(["u9_n2"])
    );
}

#[test]
fn relays_preserve_modern_inbounds_acme_and_determinism() {
    let entry = node(
        1,
        vec![Access {
            user_id: 7,
            uuid: Uuid::from_u128(7),
            credential: String::new(),
        }],
    );
    let mut modern = node(
        3,
        vec![Access {
            user_id: 9,
            uuid: Uuid::from_u128(9),
            credential: STANDARD.encode([9; 32]),
        }],
    );
    modern.port = 444;
    modern.sni = "tls.example.com".into();
    modern.protocol_config = ProtocolConfig::Anytls {
        tls: TlsConfig::Acme {
            email: "ops@example.com".into(),
            challenge: AcmeChallenge::Http01,
        },
    };
    let nodes = [entry.clone(), modern.clone()];
    let plain: Value = serde_json::from_str(&compile_server(&nodes).unwrap()).unwrap();
    let compiled = compile_server_with_relays(&nodes, &[relay(1)]).unwrap();
    let reversed = compile_server_with_relays(&[modern, entry], &[relay(1)]).unwrap();
    assert_eq!(compiled, reversed);
    let config: Value = serde_json::from_str(&compiled).unwrap();
    assert_eq!(config["inbounds"], plain["inbounds"]);
    assert_eq!(
        config["certificate_providers"],
        plain["certificate_providers"]
    );
    assert_eq!(config["inbounds"][1]["type"], "anytls");
    assert_eq!(
        config["inbounds"][1]["users"][0]["password"],
        STANDARD.encode([9; 32])
    );
    assert_eq!(
        config["inbounds"][1]["tls"]["certificate_provider"],
        "managed-tls"
    );
    assert_eq!(
        config["certificate_providers"][0]["domain"],
        serde_json::json!(["tls.example.com"])
    );
    assert_eq!(
        config["route"]["rules"][0]["inbound"],
        serde_json::json!(["node-1"])
    );
}

#[test]
fn relays_reject_non_reality_endpoints() {
    for endpoint in [1, 2] {
        let mut modern = node(endpoint, vec![]);
        modern.sni.clear();
        modern.protocol_config = ProtocolConfig::Shadowsocks2022 {
            method: SsMethod::Aes128,
            password: STANDARD.encode([9; 16]),
        };
        assert!(compile_server(std::slice::from_ref(&modern)).is_ok());
        assert!(matches!(
            compile_server_with_relays(&[modern], &[relay(1)]),
            Err(apsp_compiler::CompileError::InvalidNode { .. })
        ));
    }
}

#[test]
#[ignore = "requires the pinned upstream binary in APSP_GROUPS_RUNTIME"]
fn pinned_native_runtime_accepts_entry_exit_and_client_configs()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = std::env::var("APSP_GROUPS_RUNTIME")?;
    let version = std::process::Command::new(&runtime)
        .arg("version")
        .output()?;
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).contains("sing-box version 1.14.2"));
    let directory = std::env::temp_dir().join(format!("apsp-relay-native-{}", Uuid::new_v4()));
    std::fs::create_dir(&directory)?;
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(directory.clone());
    let entry = node(
        1,
        vec![Access {
            user_id: 7,
            uuid: Uuid::from_u128(7),
            credential: String::new(),
        }],
    );
    let exit = node(2, vec![]);
    let relay = relay(1);
    for (name, config) in [
        (
            "entry",
            compile_server_with_relays(std::slice::from_ref(&entry), std::slice::from_ref(&relay))?,
        ),
        ("exit", compile_server_with_relays(&[exit], &[relay])?),
        ("client", compile_client(&[entry], 7)?),
    ] {
        let path = directory.join(format!("{name}.json"));
        std::fs::write(&path, config)?;
        let output = std::process::Command::new(&runtime)
            .arg("check")
            .arg("-c")
            .arg(&path)
            .output()?;
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

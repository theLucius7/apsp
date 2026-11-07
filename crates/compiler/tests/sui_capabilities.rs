#![forbid(unsafe_code)]
use apsp_compiler::{
    Access, Node, ProtocolConfig, PublicAddress, TlsConfig, compile_client, compile_server,
    subscription_links,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use uuid::Uuid;
fn node(id: i64, protocol_config: ProtocolConfig) -> Node {
    Node {
        id,
        name: format!("Tokyo {id}"),
        port: 21000 + id as u16,
        public_host: "proxy.example.com".into(),
        sni: if protocol_config.tls().is_some()
            || matches!(protocol_config, ProtocolConfig::Shadowtls { .. })
        {
            "proxy.example.com".into()
        } else {
            String::new()
        },
        private_key: String::new(),
        public_key: String::new(),
        short_id: String::new(),
        users: vec![Access {
            user_id: 1,
            uuid: Uuid::from_u128(1),
            credential: STANDARD.encode(vec![1; protocol_config.credential_size()]),
        }],
        enabled: true,
        settings: Default::default(),
        protocol_config,
    }
}
fn dns() -> TlsConfig {
    TlsConfig::Dns01 {
        email: "admin@example.com".into(),
        api_token: "TEST_ONLY-cloudflare-credential".into(),
        domains: vec!["*.example.com".into()],
    }
}
#[test]
fn ordinary_protocols_and_addresses_keep_single_server_identity() {
    let mut nodes = vec![
        node(1, ProtocolConfig::VlessTls { tls: dns() }),
        node(2, ProtocolConfig::Trojan { tls: dns() }),
        node(3, ProtocolConfig::Vmess { tls: dns() }),
        node(
            4,
            ProtocolConfig::Shadowsocks {
                method: "aes-128-gcm".into(),
            },
        ),
        node(5, ProtocolConfig::Http { tls: dns() }),
        node(6, ProtocolConfig::Socks),
        node(7, ProtocolConfig::Mixed { tls: dns() }),
    ];
    nodes[0].settings.public_addresses.push(PublicAddress {
        name: "IPv6".into(),
        server: "2001:db8::1".into(),
        port: 443,
        sni: Some("proxy.example.com".into()),
        host: None,
    });
    let server: Value = serde_json::from_str(&compile_server(&nodes).unwrap()).unwrap();
    assert_eq!(server["inbounds"].as_array().unwrap().len(), 7);
    assert_eq!(server["certificate_providers"].as_array().unwrap().len(), 1);
    assert!(
        server["inbounds"][0]["tls"]["certificate_provider"]
            == server["inbounds"][1]["tls"]["certificate_provider"]
    );
    let client = compile_client(&nodes, 1).unwrap();
    assert!(!client.contains("TEST_ONLY-cloudflare"));
    let client: Value = serde_json::from_str(&client).unwrap();
    assert_eq!(
        client["outbounds"][0]["outbounds"]
            .as_array()
            .unwrap()
            .len(),
        8
    );
    let links = String::from_utf8(
        STANDARD
            .decode(subscription_links(&nodes, 1).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert!(links.contains("[2001:db8::1]:443"));
    assert!(links.contains("security=tls"));
    assert!(!links.contains("security=reality"));
}
#[test]
fn shadowtls_keeps_accounting_at_the_inner_proxy() {
    let nodes = vec![node(
        1,
        ProtocolConfig::Shadowtls {
            method: "chacha20-ietf-poly1305".into(),
        },
    )];
    let server: Value = serde_json::from_str(&compile_server(&nodes).unwrap()).unwrap();
    assert_eq!(server["inbounds"][0]["type"], "shadowtls");
    assert_eq!(
        server["inbounds"][0]["detour"],
        server["inbounds"][1]["tag"]
    );
    assert!(server["inbounds"][1].get("listen_port").is_none());
    let client: Value = serde_json::from_str(&compile_client(&nodes, 1).unwrap()).unwrap();
    assert_eq!(client["outbounds"][0]["outbounds"], json!(["node-1"]));
    assert_eq!(client["outbounds"][1]["detour"], "node-1-shadowtls");
    assert!(subscription_links(&nodes, 1).is_err());
}
#[test]
fn realm_client_does_not_receive_the_server_token() {
    let mut n = node(1, ProtocolConfig::Hysteria2 { tls: dns() });
    n.settings.realm = Some(apsp_compiler::HysteriaRealm {
        server_url: "https://realm.example.com".into(),
        realm_id: "test".into(),
        server_token: "TEST_ONLY-server-token-00000000000000".into(),
        client_token: "TEST_ONLY-client-token-00000000000000".into(),
        stun_servers: vec!["stun.example.com:3478".into()],
        ip_version: 0,
    });
    let output = compile_client(&[n], 1).unwrap();
    assert!(output.contains("TEST_ONLY-client-token"));
    assert!(!output.contains("TEST_ONLY-server-token"));
}

#[test]
#[ignore = "Requires local patched runtime and PEM fixture; set APSP_TEST_SINGBOX, APSP_TEST_CERT, APSP_TEST_KEY"]
fn native_extended_protocols_and_services_check() {
    use std::{fs, process::Command};
    let binary = std::env::var("APSP_TEST_SINGBOX").unwrap();
    let tls = TlsConfig::Manual {
        certificate: fs::read_to_string(std::env::var("APSP_TEST_CERT").unwrap()).unwrap(),
        key: fs::read_to_string(std::env::var("APSP_TEST_KEY").unwrap()).unwrap(),
    };
    let definitions = vec![
        ProtocolConfig::VlessTls { tls: tls.clone() },
        ProtocolConfig::Trojan { tls: tls.clone() },
        ProtocolConfig::Vmess { tls: tls.clone() },
        ProtocolConfig::Http { tls: tls.clone() },
        ProtocolConfig::Mixed { tls: tls.clone() },
        ProtocolConfig::Socks,
        ProtocolConfig::Shadowsocks {
            method: "aes-128-gcm".into(),
        },
        ProtocolConfig::Shadowtls {
            method: "chacha20-ietf-poly1305".into(),
        },
        ProtocolConfig::Naive { tls: tls.clone() },
    ];
    let nodes: Vec<_> = definitions
        .into_iter()
        .enumerate()
        .map(|(i, p)| node(i as i64 + 1, p))
        .collect();
    let root = std::env::temp_dir().join(format!("apsp-native-sui-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let server = compile_server(&nodes).unwrap();
    for (name, content) in [
        ("server", server.clone()),
        ("client", compile_client(&nodes, 1).unwrap()),
    ] {
        let path = root.join(format!("{name}.json"));
        fs::write(&path, content).unwrap();
        let output = Command::new(&binary)
            .args(["check", "-c"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let native: Value = serde_json::from_str(&server).unwrap();
    let profile:apsp_compiler::server_profile::ServerProfile=serde_json::from_value(json!({"services":[{"type":"hysteria-realm","tag":"extra-realm","listen":"127.0.0.1","listen_port":22000,"tls":native["inbounds"][0]["tls"],"users":[{"name":"test","token":"TEST_ONLY-realmtoken-00000000000000000"}]}],"outbounds":[{"type":"direct","tag":"extra-direct"}],"route":{"rules":[{"domain_suffix":["example.org"],"outbound":"extra-direct"}],"final":"direct"}})).unwrap();
    let path = root.join("profile.json");
    fs::write(&path, profile.apply(&server).unwrap()).unwrap();
    let output = Command::new(&binary)
        .args(["check", "-c"])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn links_never_drop_manual_trust_or_advanced_session_options() {
    let tls = TlsConfig::Manual {
        certificate: "-----BEGIN CERTIFICATE-----\nTEST_ONLY\n-----END CERTIFICATE-----".into(),
        key: "-----BEGIN PRIVATE KEY-----\nTEST_ONLY\n-----END PRIVATE KEY-----".into(),
    };
    let n = node(1, ProtocolConfig::Vmess { tls });
    assert!(subscription_links(&[n], 1).is_err());
    let mut n = node(1, ProtocolConfig::Hysteria2 { tls: dns() });
    n.settings.hysteria2.up_mbps = Some(10);
    n.settings.hysteria2.down_mbps = Some(20);
    assert!(subscription_links(&[n], 1).is_err());
}

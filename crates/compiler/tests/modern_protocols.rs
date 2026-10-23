#![forbid(unsafe_code)]

use apsp_compiler::{
    Access, AcmeChallenge, Node, ProtocolConfig, SsMethod, TlsConfig, compile_client,
    compile_server, subscription_links,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use uuid::Uuid;

#[path = "modern_protocols/advanced.rs"]
mod advanced;

fn nodes() -> Vec<Node> {
    let tls = TlsConfig::Acme {
        email: "admin@example.com".into(),
        challenge: AcmeChallenge::Http01,
    };
    [
        ProtocolConfig::Hysteria2 { tls: tls.clone() },
        ProtocolConfig::Shadowsocks2022 {
            method: SsMethod::Aes128,
            password: STANDARD.encode([11; 16]),
        },
        ProtocolConfig::Shadowsocks2022 {
            method: SsMethod::Aes256,
            password: STANDARD.encode([12; 32]),
        },
        ProtocolConfig::Tuic { tls: tls.clone() },
        ProtocolConfig::Anytls { tls: tls.clone() },
        ProtocolConfig::Naive { tls },
        ProtocolConfig::SnellV6 {
            psk: STANDARD.encode([13; 32]),
        },
    ]
    .into_iter()
    .enumerate()
    .map(|(index, protocol_config)| Node {
        enabled: true,
        settings: Default::default(),
        id: index as i64 + 1,
        name: format!("Test {index}"),
        port: 20100 + index as u16,
        public_host: "proxy.example.com".into(),
        sni: if protocol_config.tls().is_some() {
            "proxy.example.com".into()
        } else {
            String::new()
        },
        private_key: String::new(),
        public_key: String::new(),
        short_id: String::new(),
        users: [1, 2]
            .map(|user_id| Access {
                user_id,
                uuid: Uuid::from_u128(user_id as u128),
                credential: STANDARD.encode(vec![user_id as u8; protocol_config.credential_size()]),
            })
            .to_vec(),
        protocol_config,
    })
    .collect()
}

#[test]
fn all_protocols_preserve_identity_and_separate_client_secrets() {
    let nodes = nodes();
    let server: Value = serde_json::from_str(&compile_server(&nodes).unwrap()).unwrap();
    let client_text = compile_client(&nodes, 1).unwrap();
    let client: Value = serde_json::from_str(&client_text).unwrap();
    let inbounds = server["inbounds"].as_array().unwrap();
    assert_eq!(inbounds.len(), 7);
    assert_eq!(
        server["certificate_providers"][0]["domain"],
        json!(["proxy.example.com"])
    );
    assert_eq!(
        server["certificate_providers"][0]["data_directory"],
        "certificates"
    );
    assert_eq!(
        server["experimental"]["v2ray_api"]["stats"]["users"]
            .as_array()
            .unwrap()
            .len(),
        14
    );
    for (node, inbound) in nodes.iter().zip(inbounds) {
        let field = if node.protocol_config.kind() == "naive" {
            "username"
        } else {
            "name"
        };
        assert_eq!(inbound["users"][0][field], format!("u1_n{}", node.id));
        assert!(!client_text.contains(&node.users[1].credential));
        assert!(!client_text.contains(&node.users[1].uuid.to_string()));
        assert!(!client_text.contains("managed-tls"));
        assert!(!client_text.contains("admin@example.com"));
    }
    assert_eq!(
        client["outbounds"][2]["password"],
        format!("{}:{}", STANDARD.encode([11; 16]), STANDARD.encode([1; 16]))
    );
    assert_eq!(client["outbounds"][7]["version"], 6);
    assert_eq!(
        client["outbounds"][7]["userkey"],
        nodes[6].users[0].credential
    );
    assert!(
        subscription_links(&nodes, 1)
            .unwrap_err()
            .to_string()
            .contains("JSON")
    );
}

#[test]
fn deterministic_and_empty_nodes_do_not_request_certificates() {
    let mut reversed = nodes();
    reversed.reverse();
    for node in &mut reversed {
        node.users.reverse();
    }
    assert_eq!(
        compile_server(&nodes()).unwrap(),
        compile_server(&reversed).unwrap()
    );
    assert_eq!(
        compile_client(&nodes(), 1).unwrap(),
        compile_client(&reversed, 1).unwrap()
    );
    for node in &mut reversed {
        node.users.clear();
    }
    assert_eq!(
        compile_server(&reversed).unwrap(),
        compile_server(&[]).unwrap()
    );
}

#[test]
fn invalid_secrets_and_conflicting_acme_settings_are_rejected() {
    let mut model = nodes();
    model[0].users[1].credential = model[0].users[0].credential.clone();
    assert!(compile_server(&model).is_err());
    let mut model = nodes();
    model[1].users[0].credential = STANDARD.encode([0; 32]);
    assert!(compile_server(&model).is_err());
    let mut model = nodes();
    model[3].protocol_config = ProtocolConfig::Tuic {
        tls: TlsConfig::Acme {
            email: "other@example.com".into(),
            challenge: AcmeChallenge::Http01,
        },
    };
    assert!(compile_server(&model).is_err());
    let mut model = nodes();
    model[1].port = 80;
    assert!(compile_server(&model).is_err());
    model[1].port = 443;
    assert!(compile_server(&model).is_ok());
    // A UDP listener can share the HTTP challenge's numeric port.
    model[0].port = 80;
    assert!(compile_server(&model).is_ok());
}

#[test]
fn manual_certificate_private_key_never_enters_client_config() {
    let mut model = nodes();
    model[0].protocol_config = ProtocolConfig::Hysteria2 { tls: TlsConfig::Manual {
        certificate: "-----BEGIN CERTIFICATE-----\nsynthetic-test-only\n-----END CERTIFICATE-----".into(),
        key: "-----BEGIN PRIVATE KEY-----\nsynthetic-private-test-only\n-----END PRIVATE KEY-----".into(),
    }};
    let server = compile_server(&model).unwrap();
    let client = compile_client(&model, 1).unwrap();
    assert!(server.contains("synthetic-private-test-only"));
    assert!(!client.contains("synthetic-private-test-only"));
    assert!(client.contains("synthetic-test-only"));
    assert!(!client.contains("insecure"));
}

fn configured_nodes() -> Vec<Node> {
    let mut model = nodes();
    model[0].settings.hysteria2 = apsp_compiler::Hysteria2Settings {
        up_mbps: Some(80),
        down_mbps: Some(40),
        ignore_client_bandwidth: false,
        obfs_password: Some("synthetic-obfs-fixture".into()),
        ..Default::default()
    };
    model[0].settings.tls_alpn = vec!["h3".into()];
    model[3].settings.tuic = apsp_compiler::TuicSettings {
        congestion_control: apsp_compiler::CongestionControl::Bbr,
        auth_timeout_seconds: Some(5),
        heartbeat_seconds: Some(8),
        zero_rtt_handshake: true,
        ..Default::default()
    };
    model[4].settings.anytls = apsp_compiler::AnyTlsSettings {
        idle_session_check_seconds: Some(10),
        idle_session_timeout_seconds: Some(20),
        min_idle_session: Some(2),
        ..Default::default()
    };
    model[4].settings.tcp_fast_open = true;
    model[5].settings.tls_alpn = vec!["h2".into()];
    model
}

#[test]
fn protocol_settings_match_endpoint_direction_and_client_server_boundaries() {
    let model = configured_nodes();
    let server: Value = serde_json::from_str(&compile_server(&model).unwrap()).unwrap();
    let client: Value = serde_json::from_str(&compile_client(&model, 1).unwrap()).unwrap();
    assert_eq!(server["inbounds"][0]["up_mbps"], 80);
    assert_eq!(client["outbounds"][1]["up_mbps"], 40);
    assert_eq!(client["outbounds"][1]["down_mbps"], 80);
    assert_eq!(
        client["outbounds"][1]["obfs"],
        server["inbounds"][0]["obfs"]
    );
    assert_eq!(server["inbounds"][3]["congestion_control"], "bbr");
    assert_eq!(client["outbounds"][4]["congestion_control"], "bbr");
    assert_eq!(server["inbounds"][3]["auth_timeout"], "5s");
    assert!(client["outbounds"][4].get("auth_timeout").is_none());
    assert_eq!(client["outbounds"][4]["heartbeat"], "8s");
    assert_eq!(client["outbounds"][4]["zero_rtt_handshake"], true);
    assert_eq!(client["outbounds"][5]["idle_session_check_interval"], "10s");
    assert_eq!(client["outbounds"][5]["idle_session_timeout"], "20s");
    assert_eq!(client["outbounds"][5]["min_idle_session"], 2);
    assert!(server["inbounds"][4].get("idle_session_timeout").is_none());
    assert_eq!(server["inbounds"][4]["tcp_fast_open"], true);
    assert!(client["outbounds"][5].get("tcp_fast_open").is_none());
    assert_eq!(server["inbounds"][5]["tls"]["alpn"], json!(["h2"]));
    assert!(client["outbounds"][6]["tls"].get("alpn").is_none());
    assert!(matches!(
        compile_client(&model, 99),
        Err(apsp_compiler::CompileError::NoAuthorizedNodes(99))
    ));
    let mut paused = model.clone();
    for node in &mut paused {
        node.enabled = false;
    }
    assert_eq!(
        compile_server(&paused).unwrap(),
        compile_server(&[]).unwrap()
    );
    let mut reversed = model.clone();
    reversed.reverse();
    assert_eq!(
        compile_server(&model).unwrap(),
        compile_server(&reversed).unwrap()
    );
}

#[test]
fn invalid_settings_and_cross_protocol_options_are_rejected() {
    for settings in [
        json!({"listen":"https://example.com"}),
        json!({"listen":"224.0.0.1"}),
        json!({"public_port":0}),
        json!({"tcp_fast_open":true}),
        json!({"tls_alpn":["h3","h3"]}),
        json!({"tls_alpn":["bad\nvalue"]}),
        json!({"hysteria2":{"up_mbps":10}}),
        json!({"hysteria2":{"up_mbps":10,"down_mbps":10,"ignore_client_bandwidth":true}}),
        json!({"hysteria2":{"obfs_password":"short"}}),
        json!({"tuic":{"heartbeat_seconds":1}}),
        json!({"reality":{"handshake_port":8443}}),
        json!({"anytls":{"min_idle_session":1}}),
    ] {
        let mut model = nodes();
        model[0].settings = serde_json::from_value(settings.clone()).unwrap();
        assert!(compile_server(&model).is_err(), "accepted {settings}");
        assert!(
            compile_client(&model, 1).is_err(),
            "accepted client {settings}"
        );
    }
    let mut model = nodes();
    model[5].settings.tls_alpn = vec!["h3".into()];
    assert!(compile_server(&model).is_err());
    assert!(serde_json::from_value::<apsp_compiler::NodeSettings>(json!({"raw_json":{}})).is_err());
}

#[test]
#[ignore = "requires the official v1.14.2 binary in APSP_TEST_UPSTREAM; checks protocol fields without the optional accounting extension"]
fn official_runtime_accepts_protocol_settings() {
    let binary = std::env::var("APSP_TEST_UPSTREAM").unwrap();
    let directory = std::env::temp_dir().join(format!("apsp-settings-check-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let mut model = advanced::configured_nodes();
    // The official archive omits with_v2ray_api. Its native parser still
    // validates all newly introduced inbound and outbound protocol fields.
    for variant in 0..4 {
        let force_bbr = variant > 0;
        if force_bbr {
            model[0].settings.hysteria2.up_mbps = None;
            model[0].settings.hysteria2.down_mbps = None;
            model[0].settings.hysteria2.ignore_client_bandwidth = true;
        }
        if variant == 1 {
            model[0].settings.hysteria2.bbr_profile = apsp_compiler::BbrProfile::Aggressive;
            model[3].settings.tuic.udp_relay_mode = apsp_compiler::TuicUdpRelayMode::QuicStream;
            model[6].settings.snell.mode = apsp_compiler::SnellMode::UnsafeRaw;
            let mux = &mut model[1].settings.shadowsocks.multiplex;
            mux.protocol = apsp_compiler::MultiplexProtocol::Yamux;
            mux.max_connections = None;
            mux.min_streams = None;
            mux.max_streams = Some(8);
        } else if variant == 2 {
            model[0].settings.hysteria2.bbr_profile = apsp_compiler::BbrProfile::Standard;
            model[3].settings.tuic.udp_relay_mode = apsp_compiler::TuicUdpRelayMode::Native;
            model[6].settings.snell.mode = apsp_compiler::SnellMode::Default;
            model[1].settings.shadowsocks.multiplex.protocol =
                apsp_compiler::MultiplexProtocol::H2mux;
            model[4].settings.anytls.padding_scheme = vec!["stop=8".into()];
            model[5].settings.tls_min_version = Some(apsp_compiler::TlsVersion::V12);
            model[5].settings.tls_max_version = Some(apsp_compiler::TlsVersion::V12);
        } else if variant == 3 {
            model[6].settings.tcp_keep_alive_seconds = None;
            model[6].settings.tcp_keep_alive_interval_seconds = None;
            model[6].settings.disable_tcp_keep_alive = true;
            let masquerade = model[0].settings.hysteria2.masquerade.as_mut().unwrap();
            masquerade.status_code = 404;
            masquerade.content_type.clear();
        }
        for (name, content) in [
            ("server", compile_server(&model).unwrap()),
            ("client", compile_client(&model, 1).unwrap()),
        ] {
            let mut native: Value = serde_json::from_str(&content).unwrap();
            native.as_object_mut().unwrap().remove("experimental");
            let path = directory.join(format!("{name}.json"));
            std::fs::write(&path, serde_json::to_vec(&native).unwrap()).unwrap();
            let output = std::process::Command::new(&binary)
                .args(["check", "-c"])
                .arg(path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{name}, variant={variant}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
#[ignore = "requires upstream v1.14.2 with QUIC, ACME, Naive and V2Ray API; set APSP_TEST_SINGBOX"]
fn upstream_accepts_every_protocol_and_acme_provider() {
    let binary = std::env::var("APSP_TEST_SINGBOX").unwrap();
    let directory = std::env::temp_dir().join(format!("apsp-modern-check-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    for (name, content) in [
        ("server", compile_server(&nodes()).unwrap()),
        ("client", compile_client(&nodes(), 1).unwrap()),
        (
            "configured-server",
            compile_server(&configured_nodes()).unwrap(),
        ),
        (
            "configured-client",
            compile_client(&configured_nodes(), 1).unwrap(),
        ),
    ] {
        let path = directory.join(format!("{name}.json"));
        std::fs::write(&path, content).unwrap();
        let output = std::process::Command::new(&binary)
            .args(["check", "-c"])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}

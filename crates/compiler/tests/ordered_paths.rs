#![forbid(unsafe_code)]

use apsp_compiler::{
    Access, ManagedAcceptance, ManagedEndpointSnapshot, Node, OrderedPath, PathCapabilities,
    PathHop, ProbeControl, ProtocolConfig, Relay, compile_client, compile_server,
    compile_server_with_paths, compile_server_with_relays, external::NormalizedOutbound,
    path_capabilities, required_build_tags, validate_path,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use uuid::Uuid;

fn node(id: i64, user: bool) -> Node {
    Node {
        id,
        name: format!("node-{id}"),
        port: 443,
        public_host: format!("node-{id}.example.com"),
        sni: "www.example.com".into(),
        private_key: URL_SAFE_NO_PAD.encode([1; 32]),
        public_key: URL_SAFE_NO_PAD.encode([2; 32]),
        short_id: "1234abcd".into(),
        users: if user {
            vec![Access {
                user_id: 7,
                uuid: Uuid::from_u128(7),
                credential: String::new(),
            }]
        } else {
            vec![]
        },
        enabled: true,
        settings: Default::default(),
        protocol_config: ProtocolConfig::VlessReality,
    }
}
fn snapshot(id: i64) -> ManagedEndpointSnapshot {
    ManagedEndpointSnapshot {
        version_id: Uuid::from_u128(1000 + id as u128),
        server_id: id,
        node: node(id, false),
    }
}
fn managed(id: i64) -> PathHop {
    PathHop::Managed {
        endpoint: Box::new(snapshot(id)),
        relay_uuid: Uuid::from_u128(2000 + id as u128),
    }
}
fn external(value: Value, identity: u128) -> PathHop {
    PathHop::External {
        source_id: 1,
        identity_epoch: 1,
        node_id: Uuid::from_u128(identity),
        version_id: Uuid::from_u128(identity + 100),
        source_revision_id: Uuid::from_u128(9000),
        outbound: serde_json::from_value(value).unwrap(),
    }
}
fn http(identity: u128) -> PathHop {
    external(
        json!({"type":"http","server":format!("http-{identity}.example.com"),"server_port":8080,
        "username":"TEST_ONLY_ACCOUNT","password":"TEST_ONLY_PASSWORD","path":"/tunnel","headers":{"X-Example":["TEST_ONLY"]}}),
        identity,
    )
}
fn path(hops: Vec<PathHop>) -> OrderedPath {
    OrderedPath {
        chain_id: 11,
        generation: 1,
        entry_node_id: 1,
        entry_server_id: 1,
        hops,
        active: true,
    }
}
fn control() -> ProbeControl {
    ProbeControl {
        listen_port: 18086,
        secret: "a".repeat(64),
    }
}
fn compile(nodes: &[Node], paths: &[OrderedPath], accepts: &[ManagedAcceptance]) -> Value {
    serde_json::from_str(
        &compile_server_with_paths(nodes, &[], paths, accepts, Some(&control())).unwrap(),
    )
    .unwrap()
}
fn acceptance(generation: u64) -> ManagedAcceptance {
    ManagedAcceptance {
        endpoint: snapshot(2),
        chain_id: 11,
        generation,
        position: 1,
        relay_uuid: Uuid::from_u128(3000 + u128::from(generation)),
    }
}

#[test]
fn legacy_bytes_and_public_subscriptions_remain_unchanged() {
    let entry = node(1, true);
    let relay = Relay {
        settings: Default::default(),
        fingerprint: Default::default(),
        chain_id: 11,
        entry_node_id: 1,
        exit_node_id: 2,
        uuid: Uuid::from_u128(99),
        public_host: "node-2.example.com".into(),
        port: 443,
        sni: "www.example.com".into(),
        public_key: URL_SAFE_NO_PAD.encode([2; 32]),
        short_id: "1234abcd".into(),
    };
    let mut historical = serde_json::to_value(&relay).unwrap();
    historical.as_object_mut().unwrap().remove("settings");
    let historical: Relay = serde_json::from_value(historical).unwrap();
    assert_eq!(
        compile_server_with_relays(std::slice::from_ref(&entry), std::slice::from_ref(&relay))
            .unwrap(),
        compile_server_with_relays(std::slice::from_ref(&entry), &[historical]).unwrap()
    );
    assert_eq!(
        compile_server(std::slice::from_ref(&entry)).unwrap(),
        compile_server_with_paths(std::slice::from_ref(&entry), &[], &[], &[], None).unwrap()
    );
    assert_eq!(
        compile_server_with_relays(std::slice::from_ref(&entry), std::slice::from_ref(&relay))
            .unwrap(),
        compile_server_with_paths(
            std::slice::from_ref(&entry),
            std::slice::from_ref(&relay),
            &[],
            &[],
            None
        )
        .unwrap()
    );
    let mut candidate = path(vec![http(31), managed(3)]);
    candidate.active = false;
    candidate.generation = 2;
    let bytes = compile_server_with_paths(
        std::slice::from_ref(&entry),
        &[relay],
        &[candidate],
        &[],
        Some(&control()),
    )
    .unwrap();
    let config: Value = serde_json::from_str(&bytes).unwrap();
    assert_eq!(
        config["route"]["rules"],
        json!([{"inbound":["node-1"],"action":"route","outbound":"chain-11"}])
    );
    assert!(
        config["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|outbound| outbound["tag"] == "chain-11-g2-h2")
    );
    let client = compile_client(&[entry], 7).unwrap();
    assert!(!client.contains("TEST_ONLY_PASSWORD"));
    assert!(!client.contains("node-3.example.com"));
    assert!(!client.contains(&control().secret));
}

#[test]
fn inactive_generation_has_no_public_fallback_and_can_be_probed_without_users() {
    let mut candidate = path(vec![http(31), managed(3)]);
    candidate.active = false;
    candidate.generation = 2;
    let config = compile(&[node(1, false)], std::slice::from_ref(&candidate), &[]);
    assert_eq!(config["inbounds"], json!([]));
    assert_eq!(
        config["route"]["rules"],
        json!([{"inbound":["node-1"],"action":"reject"}])
    );
    assert_eq!(
        config["experimental"]["clash_api"]["external_controller"],
        "127.0.0.1:18086"
    );
    assert_eq!(config["outbounds"][2]["tag"], candidate.final_tag());
    let current = path(vec![managed(2)]);
    let both = compile(&[node(1, true)], &[candidate.clone(), current.clone()], &[]);
    assert_eq!(
        both["route"]["rules"],
        json!([{"inbound":["node-1"],"action":"route","outbound":current.final_tag()}])
    );
    assert_eq!(
        both["experimental"]["v2ray_api"]["stats"]["users"],
        json!(["u7_n1"])
    );
    let mut conflicting = candidate;
    conflicting.active = true;
    assert!(
        compile_server_with_paths(&[node(1, true)], &[], &[current, conflicting], &[], None)
            .is_err()
    );
}

#[test]
fn ordered_detours_preserve_every_external_field_and_have_an_acyclic_dns_dependency() {
    let three = compile(&[node(1, true)], &[path(vec![http(31), managed(3)])], &[]);
    assert!(three["outbounds"][1].get("detour").is_none());
    assert_eq!(three["outbounds"][2]["detour"], "chain-11-g1-h1");
    assert_eq!(three["route"]["rules"][0]["outbound"], "chain-11-g1-h2");
    let original = json!({"type":"vless","server":"external.example.com","server_port":8443,"uuid":Uuid::from_u128(77).to_string(),
        "flow":"","packet_encoding":"xudp","network":["tcp","udp"],"connect_timeout":"4s","tcp_fast_open":false,"udp_fragment":true,
        "tls":{"enabled":true,"server_name":"tls.example.com","insecure":false,"alpn":["h2"],"utls":{"enabled":true,"fingerprint":"firefox"}},
        "transport":{"type":"ws","path":"/TEST_ONLY_PATH","headers":{"Host":["front.example.com"],"X-Example":["TEST_ONLY_TOKEN"]},"max_early_data":2048,"early_data_header_name":"Sec-WebSocket-Protocol"},
        "multiplex":{"enabled":true,"protocol":"smux","max_connections":2,"min_streams":1,"max_streams":8,"padding":true}});
    let ordered = path(vec![managed(2), external(original, 31), managed(3)]);
    let config = compile(&[node(1, true)], std::slice::from_ref(&ordered), &[]);
    let outbounds = config["outbounds"].as_array().unwrap();
    assert!(outbounds[1].get("detour").is_none());
    assert_eq!(outbounds[2]["detour"], "chain-11-g1-h1");
    assert_eq!(outbounds[3]["detour"], "chain-11-g1-h2");
    let PathHop::External { outbound, .. } = &ordered.hops[1] else {
        unreachable!()
    };
    let mut actual = outbounds[2].clone();
    for field in ["tag", "detour", "domain_resolver"] {
        actual.as_object_mut().unwrap().remove(field);
    }
    assert_eq!(actual, serde_json::to_value(outbound).unwrap());
    for outbound in &outbounds[1..] {
        assert_eq!(
            outbound["domain_resolver"],
            json!({"server":"chain-bootstrap"})
        );
    }
    assert_eq!(
        config["dns"],
        json!({"servers":[{"type":"local","tag":"chain-bootstrap"}],"final":"chain-bootstrap"})
    );
    assert_eq!(
        required_build_tags(&ordered),
        vec!["with_clash_api", "with_utls", "with_v2ray_api"]
    );
    let reversed = compile(&[node(1, true)], std::slice::from_ref(&ordered), &[]);
    assert_eq!(config, reversed);
}

#[test]
fn reverse_carrier_validation_distinguishes_native_udp_and_encapsulated_udp() {
    let ss = |uot: bool| {
        external(
            json!({"type":"shadowsocks","server":"ss.example.com","server_port":443,
        "method":"chacha20-ietf-poly1305","password":"TEST_ONLY_SS_PASSWORD","udp_over_tcp":{"enabled":uot,"version":2}}),
            32,
        )
    };
    assert_eq!(
        path_capabilities(&path(vec![http(31), managed(2)])).unwrap(),
        PathCapabilities {
            tcp: true,
            udp: true
        }
    );
    assert_eq!(
        path_capabilities(&path(vec![http(31)])).unwrap(),
        PathCapabilities {
            tcp: true,
            udp: false
        }
    );
    assert_eq!(
        path_capabilities(&path(vec![http(31), ss(false)])).unwrap(),
        PathCapabilities {
            tcp: true,
            udp: false
        }
    );
    assert_eq!(
        path_capabilities(&path(vec![http(31), ss(true)])).unwrap(),
        PathCapabilities {
            tcp: true,
            udp: true
        }
    );
    let socks = external(
        json!({"type":"socks","server":"socks.example.com","server_port":1080,"version":"5"}),
        33,
    );
    assert_eq!(
        path_capabilities(&path(vec![http(31), socks.clone()])).unwrap(),
        PathCapabilities {
            tcp: true,
            udp: false
        }
    );
    assert_eq!(
        path_capabilities(&path(vec![managed(2), socks])).unwrap(),
        PathCapabilities {
            tcp: true,
            udp: true
        }
    );
    let hy2 = external(
        json!({"type":"hysteria2","server":"quic.example.com","server_port":443,"password":"TEST_ONLY_QUIC_PASSWORD",
        "tls":{"enabled":true,"server_name":"quic.example.com"},"server_ports":["443:444"],"bbr_profile":"standard","disable_chrome_parrot":false}),
        34,
    );
    assert!(path_capabilities(&path(vec![http(31), hy2.clone()])).is_err());
    assert_eq!(
        path_capabilities(&path(vec![hy2, http(31)])).unwrap(),
        PathCapabilities {
            tcp: true,
            udp: false
        }
    );
    let config = compile(&[node(1, true)], &[path(vec![http(31)])], &[]);
    assert_eq!(
        config["route"]["rules"][0],
        json!({"inbound":["node-1"],"network":["udp"],"action":"reject"})
    );
    let plugin = external(
        json!({"type":"shadowsocks","server":"plugin.example.com","server_port":443,"method":"aes-128-gcm",
        "password":"TEST_ONLY_PLUGIN_PASSWORD","plugin":"v2ray-plugin","plugin_opts":"host=plugin.example.com;mode=quic;mux=1;path=/;tls"}),
        35,
    );
    assert!(path_capabilities(&path(vec![http(31), plugin.clone()])).is_err());
    assert!(required_build_tags(&path(vec![plugin])).contains(&"with_quic".to_string()));
}

#[test]
fn internal_acceptances_share_frozen_listeners_without_changing_accounting() {
    let mut live = node(2, true);
    live.name = "Renamed endpoint".into();
    let a = acceptance(1);
    let b = acceptance(2);
    let config = compile(std::slice::from_ref(&live), &[], &[b.clone(), a.clone()]);
    assert_eq!(config["inbounds"].as_array().unwrap().len(), 1);
    assert_eq!(config["inbounds"][0]["users"].as_array().unwrap().len(), 3);
    assert_eq!(config["inbounds"][0]["users"][1]["name"], "relay_11_g1_h1");
    assert_eq!(
        config["experimental"]["v2ray_api"]["stats"]["users"],
        json!(["u7_n2"])
    );
    assert_eq!(
        config["route"]["rules"][0]["auth_user"],
        json!(["relay_11_g1_h1"])
    );
    assert_eq!(config, compile(&[live.clone()], &[], &[a.clone(), b]));
    assert!(
        compile_server_with_paths(&[live.clone()], &[], &[], &[a.clone(), a.clone()], None)
            .is_err()
    );
    live.port = 444;
    assert!(compile_server_with_paths(&[live], &[], &[], std::slice::from_ref(&a), None).is_err());
    let restored = compile(&[], &[], &[a]);
    assert_eq!(restored["inbounds"][0]["listen_port"], 443);
    assert_eq!(
        restored["experimental"]["v2ray_api"]["stats"]["users"],
        json!([])
    );
}

#[test]
fn ordered_managed_transports_keep_private_acceptance_flow_and_required_features() {
    for transport in [
        json!({"type":"tcp"}),
        json!({"type":"ws","path":"/private","host":"edge.example.com","max_early_data":1024,"early_data_header_name":"Sec-WebSocket-Protocol"}),
        json!({"type":"httpupgrade","path":"/private","host":"edge.example.com"}),
        json!({"type":"grpc","service_name":"private.service"}),
    ] {
        let mut frozen = snapshot(2);
        frozen.node.settings = serde_json::from_value(json!({
            "reality":{"flow":"none"},"transport":transport,"public_port":8443
        }))
        .unwrap();
        let identity = Uuid::from_u128(3001);
        let ordered = path(vec![PathHop::Managed {
            endpoint: Box::new(frozen.clone()),
            relay_uuid: identity,
        }]);
        let accept = ManagedAcceptance {
            endpoint: frozen.clone(),
            chain_id: ordered.chain_id,
            generation: ordered.generation,
            position: 1,
            relay_uuid: identity,
        };
        let entry = compile(&[node(1, true)], std::slice::from_ref(&ordered), &[]);
        let outgoing = entry["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|value| value["tag"] == "chain-11-g1-h1")
            .unwrap();
        assert_eq!(outgoing["server_port"], 8443);
        assert_eq!(outgoing["uuid"], identity.to_string());
        assert!(outgoing.get("flow").is_none());
        assert_eq!(
            required_build_tags(&ordered).contains(&"with_grpc".to_string()),
            transport["type"] == "grpc"
        );
        let mut live = frozen.node.clone();
        live.users = node(2, true).users;
        let direct: Value =
            serde_json::from_str(&compile_client(&[live.clone()], 7).unwrap()).unwrap();
        assert_eq!(outgoing["transport"], direct["outbounds"][1]["transport"]);
        assert!(
            !serde_json::to_string(&direct)
                .unwrap()
                .contains(&identity.to_string())
        );
        for local in [vec![live], vec![]] {
            let exit = compile(&local, &[], std::slice::from_ref(&accept));
            let identities = exit["inbounds"][0]["users"].as_array().unwrap();
            let internal = identities
                .iter()
                .find(|user| user["uuid"] == identity.to_string())
                .unwrap();
            assert_eq!(internal["name"], "relay_11_g1_h1");
            assert!(internal.get("flow").is_none());
            assert_eq!(
                exit["experimental"]["v2ray_api"]["stats"]["users"],
                if local.is_empty() {
                    json!([])
                } else {
                    json!(["u7_n2"])
                }
            );
            assert_eq!(exit, compile(&local, &[], std::slice::from_ref(&accept)));
        }
    }
}

#[test]
fn topology_and_secondary_parameter_errors_do_not_expose_authentication() {
    let invalids = [
        path(vec![managed(1)]),
        path(vec![managed(2), managed(2)]),
        path(vec![http(31), http(31)]),
        path(vec![]),
        path((2..=10).map(managed).collect()),
        path(vec![external(
            json!({"type":"http","server":"invalid://TEST_ONLY_PASSWORD", "server_port":443,"path":"/","headers":{}}),
            31,
        )]),
    ];
    for invalid in invalids {
        let error = validate_path(&invalid).unwrap_err().to_string();
        assert!(!error.contains("TEST_ONLY_PASSWORD"));
    }
    let mut endpoint = snapshot(2);
    endpoint.node.users = node(2, true).users;
    assert!(
        validate_path(&path(vec![PathHop::Managed {
            endpoint: Box::new(endpoint),
            relay_uuid: Uuid::from_u128(21)
        }]))
        .is_err()
    );
    let mut self_external = http(31);
    let PathHop::External { outbound, .. } = &mut self_external else {
        unreachable!()
    };
    let NormalizedOutbound::Http { common, .. } = outbound.as_mut() else {
        unreachable!()
    };
    common.server = "NODE-1.EXAMPLE.COM".into();
    common.server_port = 443;
    assert!(
        compile_server_with_paths(
            &[node(1, true)],
            &[],
            &[path(vec![self_external])],
            &[],
            None
        )
        .is_err()
    );
    let bad_plugin = external(
        json!({"type":"shadowsocks","server":"ss.example.com","server_port":443,"method":"aes-128-gcm","password":"TEST_ONLY_PASSWORD",
        "plugin":"v2ray-plugin","plugin_opts":"mode=websocket;cert=/TEST_ONLY_PRIVATE_PATH"}),
        31,
    );
    let error = validate_path(&path(vec![bad_plugin]))
        .unwrap_err()
        .to_string();
    assert!(!error.contains("TEST_ONLY"));
    let custom_utls = external(
        json!({"type":"vless","server":"tls.example.com","server_port":443,"uuid":Uuid::from_u128(31).to_string(),"flow":"","packet_encoding":"xudp",
        "tls":{"enabled":true,"curve_preferences":["X25519"],"utls":{"enabled":true,"fingerprint":"chrome"}}}),
        31,
    );
    assert!(validate_path(&path(vec![custom_utls])).is_err());
    let anytls_tfo = external(
        json!({"type":"anytls","server":"tls.example.com","server_port":443,"password":"TEST_ONLY_PASSWORD","tls":{"enabled":true},"tcp_fast_open":true,
        "idle_session_check_interval":"30s","idle_session_timeout":"30s","min_idle_session":0}),
        31,
    );
    assert!(validate_path(&path(vec![anytls_tfo])).is_err());
    let mut nil_source = http(31);
    let PathHop::External { version_id, .. } = &mut nil_source else {
        unreachable!()
    };
    *version_id = Uuid::nil();
    assert!(validate_path(&path(vec![nil_source])).is_err());
    let mut colliding = node(1, true);
    colliding.port = 18086;
    assert!(
        compile_server_with_paths(
            &[colliding],
            &[],
            &[path(vec![managed(2)])],
            &[],
            Some(&control())
        )
        .is_err()
    );
    let mut nil = path(vec![managed(2)]);
    nil.generation = 0;
    assert!(path_capabilities(&nil).is_err());
}

#[test]
fn all_normalized_protocols_round_trip_and_versions_do_not_change_tags() {
    let uuid = Uuid::from_u128(77).to_string();
    let protocols = vec![
        json!({"type":"shadowsocks","method":"2022-blake3-aes-128-gcm","password":"AQEBAQEBAQEBAQEBAQEBAQ=="}),
        json!({"type":"vmess","uuid":uuid,"security":"aes-128-gcm","alter_id":0,"global_padding":true,"authenticated_length":true,"packet_encoding":"xudp"}),
        json!({"type":"trojan","password":"TEST_ONLY_TROJAN_PASSWORD","tls":{"enabled":true}}),
        json!({"type":"vless","uuid":uuid,"flow":"","packet_encoding":"xudp"}),
        json!({"type":"hysteria2","password":"TEST_ONLY_HY2_PASSWORD","tls":{"enabled":true},"server_ports":["443:444"],"hop_interval":"5s","up_mbps":20,"down_mbps":40,
            "bbr_profile":"conservative","disable_chrome_parrot":true,"obfs":{"type":"salamander","password":"TEST_ONLY_OBFS_PASSWORD"}}),
        json!({"type":"tuic","uuid":uuid,"password":"TEST_ONLY_TUIC_PASSWORD","tls":{"enabled":true},"congestion_control":"bbr","udp_relay_mode":"native","udp_over_stream":false,
            "zero_rtt_handshake":false,"heartbeat":"8s"}),
        json!({"type":"anytls","password":"TEST_ONLY_ANYTLS_PASSWORD","tls":{"enabled":true},"idle_session_check_interval":"20s","idle_session_timeout":"30s","min_idle_session":1,
            "client_metadata":"TEST_ONLY_METADATA"}),
        json!({"type":"socks","version":"5","username":"TEST_ONLY_ACCOUNT","password":"TEST_ONLY_SOCKS_PASSWORD","udp_over_tcp":{"enabled":true,"version":2}}),
        json!({"type":"http","path":"/connect","headers":{"X-Example":["TEST_ONLY_TOKEN"]},"tls":{"enabled":true}}),
    ];
    for (index, mut value) in protocols.into_iter().enumerate() {
        value["server"] = json!(format!("protocol-{index}.example.com"));
        value["server_port"] = json!(443);
        let ordered = path(vec![external(value, 31)]);
        validate_path(&ordered).unwrap();
        let config = compile(&[node(1, true)], std::slice::from_ref(&ordered), &[]);
        let PathHop::External { outbound, .. } = &ordered.hops[0] else {
            unreachable!()
        };
        let mut emitted = config["outbounds"][1].clone();
        for field in ["tag", "domain_resolver"] {
            emitted.as_object_mut().unwrap().remove(field);
        }
        assert_eq!(emitted, serde_json::to_value(outbound).unwrap());
        let mut new_version = ordered.clone();
        let PathHop::External {
            version_id,
            source_revision_id,
            ..
        } = &mut new_version.hops[0]
        else {
            unreachable!()
        };
        *version_id = Uuid::from_u128(9991);
        *source_revision_id = Uuid::from_u128(9992);
        assert_eq!(ordered.final_tag(), new_version.final_tag());
        assert_ne!(
            serde_json::to_value(&ordered).unwrap(),
            serde_json::to_value(&new_version).unwrap()
        );
    }
}

#[test]
fn numeric_and_uuid_paths_share_one_configuration_without_losing_routes_or_dns() {
    use apsp_compiler::{compile_server_with_paths_on_config, paths as numeric};
    let ordered_entry = node(1, true);
    let mut numeric_entry = node(5, true);
    numeric_entry.port = 8443;
    let nodes = vec![ordered_entry, numeric_entry];
    let old = numeric::Path {
        chain_id: 22,
        generation: 3,
        entry_server_id: 1,
        entry_node_id: 5,
        active: true,
        hops: vec![numeric::Hop::External {
            node_id: 51,
            version_id: 52,
            outbound: apsp_compiler::external::ExternalOutbound(
                json!({"type":"http","server":"numeric.example.com","server_port":8080}),
            ),
        }],
    };
    let old_control = numeric::Control {
        secret: control().secret,
        test_url: "https://panel.example.com/health".into(),
    };
    let base = numeric::compile(
        1,
        &nodes,
        &[],
        &[old],
        &[],
        Default::default(),
        Some(&old_control),
    )
    .unwrap();
    let constraints = serde_json::to_value(&base.constraints).unwrap();
    let checks = serde_json::to_value(&base.checks).unwrap();
    let ordered = path(vec![http(31)]);
    let merged: Value = serde_json::from_str(
        &compile_server_with_paths_on_config(&nodes, &[], &[ordered], &[], Some(&control()), &base)
            .unwrap(),
    )
    .unwrap();
    let outbounds = merged["outbounds"].as_array().unwrap();
    assert!(
        outbounds
            .iter()
            .any(|outbound| outbound["tag"] == "path-22-g3-h0")
    );
    assert!(
        outbounds
            .iter()
            .any(|outbound| outbound["tag"] == "chain-11-g1-h1")
    );
    let rules = merged["route"]["rules"].as_array().unwrap();
    assert!(rules.iter().any(|rule| rule["inbound"] == json!(["node-5"]) && rule["outbound"] == "path-22-g3-h0"));
    assert!(
        rules.iter().any(
            |rule| rule["inbound"] == json!(["node-1"]) && rule["outbound"] == "chain-11-g1-h1"
        )
    );
    let dns = merged["dns"]["servers"].as_array().unwrap();
    assert!(dns.iter().any(|server| server["tag"] == "path-bootstrap"));
    assert!(dns.iter().any(|server| server["tag"] == "chain-bootstrap"));
    assert_eq!(
        merged["experimental"]["clash_api"]["secret"],
        control().secret
    );
    assert_eq!(
        serde_json::to_value(&base.constraints).unwrap(),
        constraints
    );
    assert_eq!(serde_json::to_value(&base.checks).unwrap(), checks);
    assert_eq!(
        compile_server_with_paths_on_config(&nodes, &[], &[], &[], None, &base).unwrap(),
        base.config
    );
    let mut wrong = control();
    wrong.secret = "b".repeat(64);
    assert!(
        compile_server_with_paths_on_config(
            &nodes,
            &[],
            &[path(vec![http(31)])],
            &[],
            Some(&wrong),
            &base
        )
        .is_err()
    );
}

#[test]
fn combined_pipelines_reject_competing_public_entry_and_frozen_listener_parameters() {
    use apsp_compiler::{compile_server_with_paths_on_config, paths as numeric};
    let entry = node(1, true);
    let old = numeric::Path {
        chain_id: 22,
        generation: 3,
        entry_server_id: 1,
        entry_node_id: 1,
        active: true,
        hops: vec![numeric::Hop::External {
            node_id: 51,
            version_id: 52,
            outbound: apsp_compiler::external::ExternalOutbound(
                json!({"type":"http","server":"numeric.example.com","server_port":8080}),
            ),
        }],
    };
    let old_control = numeric::Control {
        secret: control().secret,
        test_url: "https://panel.example.com/health".into(),
    };
    let base = numeric::compile(
        1,
        std::slice::from_ref(&entry),
        &[],
        &[old],
        &[],
        Default::default(),
        Some(&old_control),
    )
    .unwrap();
    assert!(
        compile_server_with_paths_on_config(
            &[entry],
            &[],
            &[path(vec![http(31)])],
            &[],
            Some(&control()),
            &base
        )
        .is_err()
    );

    let accept = acceptance(1);
    let nodes = vec![node(2, true)];
    let mut base = numeric::compile(2, &nodes, &[], &[], &[], Default::default(), None).unwrap();
    let mut native: Value = serde_json::from_str(&base.config).unwrap();
    native["inbounds"][0]["tls"]["server_name"] = json!("changed.example.com");
    base.config = serde_json::to_string(&native).unwrap();
    assert!(compile_server_with_paths_on_config(&nodes, &[], &[], &[accept], None, &base).is_err());
}

#[test]
fn converting_chain_keeps_its_mixed_route_while_its_ordered_candidate_is_probed() {
    use apsp_compiler::{compile_server_with_paths_on_config, paths as numeric};
    use std::collections::BTreeMap;
    let mut entry = node(5, true);
    entry.port = 8443;
    let nodes = vec![entry];
    let mixed = numeric::Path {
        chain_id: 22,
        generation: 3,
        entry_server_id: 1,
        entry_node_id: 5,
        active: true,
        hops: vec![numeric::Hop::External {
            node_id: 51,
            version_id: 52,
            outbound: apsp_compiler::external::ExternalOutbound(
                json!({"type":"http","server":"numeric.example.com","server_port":8080}),
            ),
        }],
    };
    let mixed_control = numeric::Control {
        secret: control().secret,
        test_url: "https://panel.example.com/health".into(),
    };
    let base = numeric::compile(
        1,
        &nodes,
        &[],
        &[mixed],
        &[],
        Default::default(),
        Some(&mixed_control),
    )
    .unwrap();
    assert_eq!(base.routes, BTreeMap::from([(5, 22)]));
    let candidate = |chain_id, active| OrderedPath {
        chain_id,
        generation: 4,
        entry_node_id: 5,
        entry_server_id: 1,
        hops: vec![http(31)],
        active,
    };
    let merged: Value = serde_json::from_str(
        &compile_server_with_paths_on_config(
            &nodes,
            &[],
            &[candidate(22, false)],
            &[],
            Some(&control()),
            &base,
        )
        .unwrap(),
    )
    .unwrap();
    let rules = merged["route"]["rules"].as_array().unwrap();
    let entry_rules: Vec<_> = rules
        .iter()
        .filter(|rule| rule["inbound"] == json!(["node-5"]))
        .collect();
    // The mixed route still carries users; the candidate adds no blanket reject
    // in front of it and no route of its own.
    assert!(
        entry_rules
            .iter()
            .any(|rule| rule["outbound"] == "path-22-g3-h0")
    );
    assert!(
        entry_rules
            .iter()
            .all(|rule| rule["action"] != "reject" || rule.get("network").is_some())
    );
    assert!(
        entry_rules
            .iter()
            .all(|rule| rule["outbound"] != "chain-22-g4-h1")
    );
    assert!(
        merged["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|outbound| outbound["tag"] == "chain-22-g4-h1")
    );
    // Routing the candidate, or another chain's candidate on the same entry,
    // would select the entry twice.
    for path in [candidate(22, true), candidate(23, false)] {
        assert!(
            compile_server_with_paths_on_config(&nodes, &[], &[path], &[], Some(&control()), &base)
                .is_err()
        );
    }
}

#![forbid(unsafe_code)]

use apsp_compiler::{
    Access, Node, NodeSettings, NodeTransport, RealityFlow, Relay, compile_client, compile_server,
    compile_server_with_relays,
    paths::{self, Control, Hop, Path},
    subscription_links,
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;

fn endpoint(id: i64) -> Node {
    Node {
        id,
        name: format!("TEST_ONLY node {id}"),
        port: 20000 + id as u16,
        public_host: format!("node-{id}.example.com"),
        sni: "www.example.com".into(),
        private_key: URL_SAFE_NO_PAD.encode([1; 32]),
        public_key: URL_SAFE_NO_PAD.encode([2; 32]),
        short_id: "1234abcd".into(),
        users: vec![Access {
            user_id: 1,
            uuid: Uuid::from_u128(1),
            credential: String::new(),
        }],
        enabled: true,
        settings: Default::default(),
        protocol_config: Default::default(),
    }
}

fn transports() -> Vec<NodeTransport> {
    [json!({"type":"tcp"}),json!({"type":"ws","path":"/proxy","host":"front.example.com","max_early_data":2048,"early_data_header_name":"Sec-WebSocket-Protocol"}),
        json!({"type":"httpupgrade","path":"/upgrade","host":"front.example.com"}),json!({"type":"grpc","service_name":"test.service"})]
        .into_iter().map(|value|serde_json::from_value(value).unwrap()).collect()
}

fn configured(transport: NodeTransport) -> Node {
    let mut node = endpoint(2);
    node.settings.transport = transport;
    node.settings.reality.flow = RealityFlow::None;
    node.settings.reality.max_time_difference_seconds = Some(60);
    node.settings.tls_handshake_timeout_seconds = Some(8);
    node.settings.public_port = Some(8443);
    node
}

#[test]
fn server_client_and_links_keep_flow_transport_and_endpoint_consistent() {
    for transport in transports() {
        let node = configured(transport.clone());
        let server: Value =
            serde_json::from_str(&compile_server(std::slice::from_ref(&node)).unwrap()).unwrap();
        let client: Value =
            serde_json::from_str(&compile_client(std::slice::from_ref(&node), 1).unwrap()).unwrap();
        let inbound = &server["inbounds"][0];
        let outbound = &client["outbounds"][1];
        assert!(inbound["users"][0].get("flow").is_none());
        assert!(outbound.get("flow").is_none());
        assert_eq!(inbound["tls"]["reality"]["max_time_difference"], "60s");
        assert_eq!(inbound["tls"]["handshake_timeout"], "8s");
        assert!(outbound["tls"].get("handshake_timeout").is_none());
        assert!(
            outbound["tls"]["reality"]
                .get("max_time_difference")
                .is_none()
        );
        assert_eq!(outbound["server_port"], 8443);
        let links = String::from_utf8(
            STANDARD
                .decode(subscription_links(std::slice::from_ref(&node), 1).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert!(!links.contains("flow="));
        assert!(links.contains(":8443?"));
        assert!(links.contains(&format!("&type={}", transport.kind())));
        match transport {
            NodeTransport::Tcp {} => {
                assert!(inbound.get("transport").is_none());
                assert!(outbound.get("transport").is_none());
            }
            NodeTransport::Ws { .. } => {
                assert!(inbound["transport"].get("headers").is_none());
                assert_eq!(
                    outbound["transport"]["headers"]["Host"],
                    "front.example.com"
                );
                assert_eq!(inbound["transport"]["max_early_data"], 2048);
                assert!(links.contains("&path=%2Fproxy"));
                assert!(links.contains("&ed=2048&eh=Sec-WebSocket-Protocol"));
            }
            NodeTransport::Httpupgrade { .. } => {
                assert_eq!(inbound["transport"], outbound["transport"]);
                assert!(links.contains("&path=%2Fupgrade&host=front.example.com"));
            }
            NodeTransport::Grpc { .. } => {
                assert_eq!(inbound["transport"], outbound["transport"]);
                assert!(links.contains("&serviceName=test.service"));
            }
        }
        assert!(!links.contains(&node.private_key));
    }
}

#[test]
fn transport_validation_rejects_vision_conflicts_and_invalid_http_inputs() {
    for settings in [
        json!({"transport":{"type":"ws"}}),
        json!({"reality":{"flow":"none"},"transport":{"type":"ws","path":"https://example.com"}}),
        json!({"reality":{"flow":"none"},"transport":{"type":"ws","path":"/proxy?ignored=1"}}),
        json!({"reality":{"flow":"none"},"transport":{"type":"httpupgrade","host":"front.example.com\r\nX: evil"}}),
        json!({"reality":{"flow":"none"},"transport":{"type":"ws","early_data_header_name":"X-Test"}}),
        json!({"reality":{"flow":"none"},"transport":{"type":"ws","max_early_data":1,"early_data_header_name":"Host"}}),
        json!({"reality":{"flow":"none"},"transport":{"type":"grpc","service_name":"a/b"}}),
        json!({"reality":{"max_time_difference_seconds":0}}),
    ] {
        let mut node = endpoint(1);
        node.settings = serde_json::from_value(settings.clone()).unwrap();
        assert!(
            compile_server(std::slice::from_ref(&node)).is_err(),
            "accepted {settings}"
        );
        assert!(compile_client(&[node], 1).is_err(), "accepted {settings}");
    }
    assert!(
        serde_json::from_value::<NodeSettings>(
            json!({"transport":{"type":"tcp","path":"/ignored"}})
        )
        .is_err()
    );
}

fn relay(node: &Node) -> Relay {
    Relay {
        settings: node.settings.clone(),
        fingerprint: node.settings.reality.fingerprint,
        chain_id: 1,
        entry_node_id: 1,
        exit_node_id: node.id,
        uuid: Uuid::from_u128(77),
        public_host: node.public_host.clone(),
        port: node.public_port(),
        sni: node.sni.clone(),
        public_key: node.public_key.clone(),
        short_id: node.short_id.clone(),
    }
}

fn path(node: &Node) -> Path {
    let mut endpoint = node.clone();
    endpoint.users.clear();
    Path {
        chain_id: 2,
        generation: 1,
        entry_server_id: 1,
        entry_node_id: 1,
        active: true,
        hops: vec![Hop::Managed {
            server_id: 2,
            endpoint: Box::new(endpoint),
            identity: Uuid::from_u128(88),
        }],
    }
}

fn mixed(server: i64, nodes: &[Node], path: Path) -> String {
    paths::compile(
        server,
        nodes,
        &[],
        &[path],
        &[],
        BTreeMap::new(),
        (server == 1).then_some(&Control {
            secret: "TEST_ONLY-control-01234567890123456789".into(),
            test_url: "https://probe.example.com/healthz".into(),
        }),
    )
    .unwrap()
    .config
}

#[test]
fn managed_and_legacy_relays_use_the_same_transport_and_private_identity() {
    for transport in transports() {
        let node = configured(transport);
        let entry = endpoint(1);
        let direct: Value =
            serde_json::from_str(&compile_client(std::slice::from_ref(&node), 1).unwrap()).unwrap();
        let outgoing: Value = serde_json::from_str(
            &compile_server_with_relays(std::slice::from_ref(&entry), &[relay(&node)]).unwrap(),
        )
        .unwrap();
        let exit: Value = serde_json::from_str(
            &compile_server_with_relays(std::slice::from_ref(&node), &[relay(&node)]).unwrap(),
        )
        .unwrap();
        assert_eq!(
            outgoing["outbounds"][1]["transport"],
            direct["outbounds"][1]["transport"]
        );
        assert!(outgoing["outbounds"][1].get("flow").is_none());
        assert_eq!(
            outgoing["outbounds"][1]["uuid"],
            Uuid::from_u128(77).to_string()
        );
        assert!(exit["inbounds"][0]["users"][1].get("flow").is_none());
        let mixed_entry: Value = serde_json::from_str(&mixed(1, &[entry], path(&node))).unwrap();
        let mixed_exit: Value =
            serde_json::from_str(&mixed(2, std::slice::from_ref(&node), path(&node))).unwrap();
        let outbound = mixed_entry["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["tag"] == "path-2-g1-h0")
            .unwrap();
        assert_eq!(outbound["transport"], direct["outbounds"][1]["transport"]);
        assert!(outbound.get("flow").is_none());
        assert_eq!(outbound["uuid"], Uuid::from_u128(88).to_string());
        assert!(mixed_exit["inbounds"][0]["users"][1].get("flow").is_none());
        assert_eq!(
            mixed_exit["experimental"]["v2ray_api"]["stats"]["users"],
            json!(["u1_n2"])
        );
        let public = compile_client(&[node], 1).unwrap();
        assert!(!public.contains(&Uuid::from_u128(77).to_string()));
        assert!(!public.contains(&Uuid::from_u128(88).to_string()));
    }
}

#[test]
#[ignore = "requires official sing-box 1.14.2 in APSP_TEST_UPSTREAM; parses configs without connecting to external hosts"]
fn official_runtime_accepts_all_transports_and_both_relay_formats() {
    let binary = std::env::var("APSP_TEST_UPSTREAM").unwrap();
    let directory = std::env::temp_dir().join(format!("apsp-transports-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    for transport in transports() {
        let node = configured(transport);
        let entry = endpoint(1);
        for (name, config) in [
            (
                "server",
                compile_server(std::slice::from_ref(&node)).unwrap(),
            ),
            (
                "client",
                compile_client(std::slice::from_ref(&node), 1).unwrap(),
            ),
            (
                "legacy-entry",
                compile_server_with_relays(std::slice::from_ref(&entry), &[relay(&node)]).unwrap(),
            ),
            (
                "legacy-exit",
                compile_server_with_relays(std::slice::from_ref(&node), &[relay(&node)]).unwrap(),
            ),
            (
                "mixed-entry",
                mixed(1, std::slice::from_ref(&entry), path(&node)),
            ),
            (
                "mixed-exit",
                mixed(2, std::slice::from_ref(&node), path(&node)),
            ),
        ] {
            let mut value: Value = serde_json::from_str(&config).unwrap();
            value.as_object_mut().unwrap().remove("experimental");
            let file = directory.join(format!("{}-{name}.json", node.settings.transport.kind()));
            std::fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
            let result = std::process::Command::new(&binary)
                .args(["check", "-c"])
                .arg(&file)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{} {name}: {}",
                node.settings.transport.kind(),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

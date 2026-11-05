#![forbid(unsafe_code)]
//! VLESS behind a TLS-terminating tunnel (ADR 0109): a plain loopback listener
//! on the server, ordinary TLS to the edge hostname on the client.

use apsp_compiler::{
    Access, Node, NodeTransport, ProtocolConfig, clash, compile_client, compile_server,
    preserved::{PreservedNode, compile_preserved_server, nexus_access},
    subscription_links,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use uuid::Uuid;

fn transport(value: Value) -> NodeTransport {
    serde_json::from_value(value).unwrap()
}

fn tunnel(id: i64) -> Node {
    let mut node = Node {
        id,
        name: format!("TEST_ONLY tunnel {id}"),
        port: 21443,
        public_host: "edge.example.com".into(),
        sni: "tunnel.example.com".into(),
        private_key: String::new(),
        public_key: String::new(),
        short_id: String::new(),
        users: vec![Access {
            user_id: 1,
            uuid: Uuid::from_u128(1),
            credential: String::new(),
        }],
        enabled: true,
        settings: Default::default(),
        protocol_config: ProtocolConfig::VlessTunnel,
    };
    node.settings.listen = "127.0.0.1".into();
    node.settings.public_port = Some(443);
    node.settings.transport = transport(json!({"type":"ws","path":"/TEST_ONLY"}));
    node
}

fn server(node: &Node) -> Value {
    serde_json::from_str(&compile_server(std::slice::from_ref(node)).unwrap()).unwrap()
}

fn client(node: &Node) -> Value {
    let value: Value =
        serde_json::from_str(&compile_client(std::slice::from_ref(node), 1).unwrap()).unwrap();
    value["outbounds"][1].clone()
}

#[test]
fn server_is_a_plain_loopback_listener_and_clients_use_tls_to_the_edge() {
    let node = tunnel(3);
    let inbound = &server(&node)["inbounds"][0];
    assert_eq!(inbound["type"], "vless");
    assert_eq!(inbound["listen"], "127.0.0.1");
    assert_eq!(inbound["listen_port"], 21443);
    assert!(inbound.get("tls").is_none());
    assert_eq!(
        inbound["transport"],
        json!({"type":"ws","path":"/TEST_ONLY"})
    );
    assert_eq!(
        inbound["users"],
        json!([{"name":"u1_n3","uuid":"00000000-0000-0000-0000-000000000001"}])
    );

    let outbound = client(&node);
    assert_eq!(outbound["type"], "vless");
    assert_eq!(outbound["server"], "edge.example.com");
    assert_eq!(outbound["server_port"], 443);
    assert_eq!(outbound["uuid"], "00000000-0000-0000-0000-000000000001");
    assert!(outbound.get("flow").is_none());
    assert_eq!(
        outbound["tls"],
        json!({"enabled":true,"server_name":"tunnel.example.com","utls":{"enabled":true,"fingerprint":"chrome"}})
    );
    // The edge routes by Host, which defaults to the tunnel hostname.
    assert_eq!(
        outbound["transport"],
        json!({"type":"ws","path":"/TEST_ONLY","headers":{"Host":"tunnel.example.com"}})
    );

    let proxy = clash::proxy(&outbound, "TEST_ONLY").unwrap();
    assert_eq!(proxy["type"], "vless");
    assert_eq!(proxy["tls"], true);
    assert_eq!(proxy["servername"], "tunnel.example.com");
    assert_eq!(proxy["network"], "ws");
    assert_eq!(
        proxy["ws-opts"],
        json!({"path":"/TEST_ONLY","headers":{"Host":"tunnel.example.com"}})
    );
    assert!(proxy.get("reality-opts").is_none());

    let links = String::from_utf8(
        STANDARD
            .decode(subscription_links(std::slice::from_ref(&node), 1).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        links,
        "vless://00000000-0000-0000-0000-000000000001@edge.example.com:443?encryption=none&security=tls&sni=tunnel.example.com&fp=chrome&type=ws&path=%2FTEST_ONLY&host=tunnel.example.com#TEST_ONLY%20tunnel%203"
    );
}

#[test]
fn explicit_hosts_and_httpupgrade_are_kept() {
    let mut node = tunnel(4);
    node.settings.transport =
        transport(json!({"type":"ws","path":"/a","host":"front.example.com"}));
    assert_eq!(
        client(&node)["transport"]["headers"],
        json!({"Host":"front.example.com"})
    );
    node.settings.transport = transport(json!({"type":"httpupgrade","path":"/b"}));
    let outbound = client(&node);
    assert_eq!(
        outbound["transport"],
        json!({"type":"httpupgrade","path":"/b","host":"tunnel.example.com"})
    );
    assert_eq!(
        server(&node)["inbounds"][0]["transport"],
        json!({"type":"httpupgrade","path":"/b"})
    );
}

#[test]
fn plain_listeners_must_stay_on_loopback_with_an_http_transport() {
    let invalid = |edit: &dyn Fn(&mut Node)| {
        let mut node = tunnel(5);
        edit(&mut node);
        compile_server(std::slice::from_ref(&node)).unwrap_err()
    };
    invalid(&|node| node.settings.listen = "::".into());
    invalid(&|node| node.settings.listen = "0.0.0.0".into());
    invalid(&|node| node.settings.listen = "192.0.2.1".into());
    invalid(&|node| node.settings.transport = NodeTransport::Tcp {});
    invalid(&|node| node.settings.transport = transport(json!({"type":"grpc","service_name":"x"})));
    invalid(&|node| node.sni = "192.0.2.1".into());
    invalid(&|node| node.sni = String::new());
    invalid(&|node| {
        node.settings.reality.flow = apsp_compiler::RealityFlow::None;
    });
    invalid(&|node| {
        let duplicate = node.users[0].clone();
        node.users.push(Access {
            user_id: 2,
            ..duplicate
        });
    });
    let mut ipv6 = tunnel(6);
    ipv6.settings.listen = "::1".into();
    assert_eq!(server(&ipv6)["inbounds"][0]["listen"], "::1");
}

fn preserved(id: i64) -> PreservedNode {
    let mut node = tunnel(id);
    node.users = vec![nexus_access(
        7,
        Uuid::parse_str("12345678-1234-4234-8234-123456789abc").unwrap(),
        &node.protocol_config,
    )];
    let mut configuration = server(&node);
    configuration
        .as_object_mut()
        .unwrap()
        .remove("experimental");
    configuration["inbounds"][0]["multiplex"] = json!({"enabled": true});
    configuration["outbounds"] =
        json!([{"tag":"direct","type":"direct"},{"tag":"block","type":"block"}]);
    configuration["route"] = json!({"final":"direct","default_domain_resolver":"local","rules":[{"ip_cidr":["10.0.0.0/8"],"outbound":"block"}]});
    configuration["dns"] = json!({"servers":[{"tag":"local","type":"udp","server":"192.0.2.53"}]});
    PreservedNode {
        node,
        configuration,
    }
}

#[test]
fn preserved_tunnel_keeps_the_listener_and_regenerates_uuid_users() {
    let text = compile_preserved_server(&[preserved(8)]).unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    let inbound = &value["inbounds"][0];
    assert_eq!(inbound["listen"], "127.0.0.1");
    assert_eq!(inbound["multiplex"], json!({"enabled": true}));
    assert_eq!(
        inbound["transport"],
        json!({"type":"ws","path":"/TEST_ONLY"})
    );
    assert_eq!(
        inbound["users"],
        json!([{"name":"u7_n8","uuid":"12345678-1234-4234-8234-123456789abc"}])
    );
    assert_eq!(value["dns"]["servers"][0]["tag"], "local");
    assert_eq!(value["route"]["default_domain_resolver"], "local");
}

#[test]
fn preserved_tunnel_refuses_a_different_listener() {
    let refuse = |edit: &dyn Fn(&mut Value)| {
        let mut input = preserved(9);
        edit(&mut input.configuration["inbounds"][0]);
        compile_preserved_server(&[input]).unwrap_err()
    };
    refuse(&|inbound| inbound["listen"] = json!("::"));
    refuse(&|inbound| inbound["transport"]["path"] = json!("/other"));
    refuse(&|inbound| inbound["transport"] = json!({"type":"httpupgrade","path":"/TEST_ONLY"}));
    refuse(&|inbound| inbound["tls"] = json!({"enabled": true, "server_name": "x.example.com"}));
}

#[test]
#[ignore = "requires official sing-box 1.14.2 in APSP_TEST_UPSTREAM; parses configs without connecting to external hosts"]
fn official_runtime_accepts_tunnel_server_and_client() {
    let binary = std::env::var("APSP_TEST_UPSTREAM").unwrap();
    let directory = std::env::temp_dir().join(format!("apsp-tunnel-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    for transport_value in [
        json!({"type":"ws","path":"/a","max_early_data":2048,"early_data_header_name":"Sec-WebSocket-Protocol"}),
        json!({"type":"httpupgrade","path":"/b"}),
    ] {
        let mut node = tunnel(10);
        node.settings.transport = transport(transport_value);
        let preserved_text = {
            let mut input = preserved(10);
            input.node.settings.transport = node.settings.transport.clone();
            input.configuration["inbounds"][0]["transport"] =
                server(&input.node)["inbounds"][0]["transport"].clone();
            compile_preserved_server(&[input]).unwrap()
        };
        for (name, config) in [
            (
                "server",
                compile_server(std::slice::from_ref(&node)).unwrap(),
            ),
            (
                "client",
                compile_client(std::slice::from_ref(&node), 1).unwrap(),
            ),
            ("preserved", preserved_text),
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

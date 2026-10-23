#![forbid(unsafe_code)]

use apsp_compiler::{Node, ProtocolConfig, Relay, compile_server, compile_server_with_relays};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use uuid::Uuid;

#[test]
fn relay_compilation_does_not_reinterpret_modern_protocols_as_reality() {
    let relay = Relay {
        settings: Default::default(),
        fingerprint: Default::default(),
        chain_id: 1,
        entry_node_id: 1,
        exit_node_id: 2,
        uuid: Uuid::from_u128(1),
        public_host: "exit.example.com".into(),
        port: 443,
        sni: "www.example.com".into(),
        public_key: URL_SAFE_NO_PAD.encode([1; 32]),
        short_id: "1234abcd".into(),
    };
    for id in [1, 2] {
        let node = Node {
            enabled: true,
            settings: Default::default(),
            id,
            name: "Modern node".into(),
            port: 443,
            public_host: "proxy.example.com".into(),
            sni: String::new(),
            private_key: String::new(),
            public_key: String::new(),
            short_id: String::new(),
            users: vec![],
            protocol_config: ProtocolConfig::SnellV6 {
                psk: STANDARD.encode([2; 32]),
            },
        };
        assert!(compile_server(std::slice::from_ref(&node)).is_ok());
        assert!(compile_server_with_relays(&[node], std::slice::from_ref(&relay)).is_err());
    }
}

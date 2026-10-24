#![forbid(unsafe_code)]
use apsp_compiler::{
    Node, ProtocolConfig, SsMethod, compile_client, compile_server,
    preserved::{PreservedNode, compile_preserved_server, nexus_access},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use uuid::Uuid;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protocol = ProtocolConfig::Shadowsocks2022 {
        method: SsMethod::Aes128,
        password: STANDARD.encode(b"0123456789abcdef"),
    };
    let node = Node {
        id: 43,
        name: "TEST_ONLY SS migration".into(),
        port: 23543,
        public_host: "127.0.0.1".into(),
        sni: String::new(),
        private_key: String::new(),
        public_key: String::new(),
        short_id: String::new(),
        enabled: true,
        settings: apsp_compiler::NodeSettings {
            listen: "127.0.0.1".into(),
            ..Default::default()
        },
        users: vec![nexus_access(
            1,
            Uuid::parse_str("12345678-1234-4234-8234-123456789abc")?,
            &protocol,
        )],
        protocol_config: protocol,
    };
    let mut before: Value = serde_json::from_str(&compile_server(std::slice::from_ref(&node))?)?;
    before.as_object_mut().unwrap().remove("experimental");
    before["inbounds"][0]["users"][0]["name"] = json!(node.users[0].uuid);
    let after: Value = serde_json::from_str(&compile_preserved_server(&[PreservedNode {
        node: node.clone(),
        configuration: before.clone(),
    }])?)?;
    let mut client: Value = serde_json::from_str(&compile_client(&[node], 1)?)?;
    client["inbounds"][0]["listen_port"] = json!(23544);
    println!("{}", json!({"before":before,"after":after,"client":client}));
    Ok(())
}

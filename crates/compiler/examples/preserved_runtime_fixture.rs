#![forbid(unsafe_code)]
//! Emit a TEST_ONLY Shadowsocks 2022 fixture for `tools/test-nexus-ss-runtime.py`.
//!
//! `legacy_input` reproduces the Xboard node builder: `shadowsocks-in` on `::`,
//! the per-node server key, users named by UUID with Base64(UUID text[..16]),
//! direct/block outbounds and the private-range block rules. The clients use
//! the Xboard subscription password `server_key:user_key`, not apsp's own
//! client compiler, so a pass proves continuity for already issued links.
use apsp_compiler::{
    Node, ProtocolConfig, SsMethod,
    preserved::{PreservedNode, compile_preserved_server, nexus_access},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use uuid::Uuid;

const KEPT: &str = "12345678-1234-4234-8234-123456789abc";
const REVOKED: &str = "87654321-4321-4321-8321-cba987654321";

fn legacy_user_key(uuid: &str) -> String {
    STANDARD.encode(&uuid.as_bytes()[..16])
}

fn client(server_key: &str, uuid: &str, socks_port: u16) -> Value {
    json!({
        "log": {"level": "warn"},
        "inbounds": [{"type": "socks", "tag": "socks-in", "listen": "127.0.0.1", "listen_port": socks_port}],
        "outbounds": [{
            "type": "shadowsocks", "tag": "proxy", "server": "127.0.0.1", "server_port": 23543,
            "method": "2022-blake3-aes-128-gcm",
            "password": format!("{server_key}:{}", legacy_user_key(uuid)),
        }],
        "route": {"final": "proxy"},
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server_key = STANDARD.encode(b"0123456789abcdef");
    let protocol = ProtocolConfig::Shadowsocks2022 {
        method: SsMethod::Aes128,
        password: server_key.clone(),
    };
    let users =
        [KEPT, REVOKED].map(|uuid| json!({"name": uuid, "password": legacy_user_key(uuid)}));
    let private_v4 = [
        "10.0.0.0/8",
        "100.64.0.0/10",
        "169.254.0.0/16",
        "172.16.0.0/12",
        "192.0.0.0/24",
        "192.168.0.0/16",
        "198.18.0.0/15",
    ];
    let inbound = json!({
        "tag": "shadowsocks-in", "listen": "127.0.0.1", "listen_port": 23543, "reuse_addr": true,
        "type": "shadowsocks", "method": "2022-blake3-aes-128-gcm", "password": server_key,
        "users": users,
    });
    // The fixture echo servers listen on loopback, so 127.0.0.0/8 is omitted
    // from the otherwise unchanged legacy block list.
    let legacy_input = json!({
        "log": {"level": "warn", "timestamp": true},
        "inbounds": [inbound],
        "outbounds": [{"type": "direct", "tag": "direct"}, {"type": "block", "tag": "block"}],
        "route": {"final": "direct", "rules": [
            {"outbound": "block", "ip_cidr": private_v4},
            {"outbound": "block", "ip_cidr": ["fc00::/7", "fe80::/10"]},
        ]},
    });
    // sing-box 1.14 removed the legacy block outbound; the control runtime uses
    // the equivalent reject action so the same listener can run before rewrite.
    let mut before = legacy_input.clone();
    before["outbounds"] = json!([{"type": "direct", "tag": "direct"}]);
    for rule in before["route"]["rules"].as_array_mut().unwrap() {
        rule.as_object_mut().unwrap().remove("outbound");
        rule["action"] = json!("reject");
    }
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
        // Only the kept member remains entitled after the takeover.
        users: vec![nexus_access(1, Uuid::parse_str(KEPT)?, &protocol)],
        protocol_config: protocol,
    };
    let after: Value = serde_json::from_str(&compile_preserved_server(&[PreservedNode {
        node,
        configuration: legacy_input,
    }])?)?;
    println!(
        "{}",
        json!({
            "before": before,
            "after": after,
            "client": client(&server_key, KEPT, 23544),
            "revoked_client": client(&server_key, REVOKED, 23547),
        })
    );
    Ok(())
}

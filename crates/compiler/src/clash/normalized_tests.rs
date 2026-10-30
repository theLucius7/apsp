use super::*;
use crate::external::{ExternalOutbound, NormalizedOutbound};

const UUID: &str = "00000000-0000-4000-8000-000000000001";

fn defaults(kind: &str) -> Value {
    let mut value = json!({
        "type": kind, "server": format!("{kind}.example.invalid"), "server_port": 443,
    });
    let fields = match kind {
        "shadowsocks" => json!({"method": "aes-128-gcm", "password": "TEST_ONLY"}),
        "vmess" => json!({
            "uuid": UUID, "security": "auto", "alter_id": 0,
            "global_padding": false, "authenticated_length": false,
        }),
        "trojan" => json!({"password": "TEST_ONLY"}),
        "vless" => json!({"uuid": UUID, "flow": "", "packet_encoding": "xudp"}),
        "hysteria2" => json!({
            "password": "TEST_ONLY", "server_ports": [],
            "bbr_profile": "standard", "disable_chrome_parrot": false,
        }),
        "tuic" => json!({
            "uuid": UUID, "password": "TEST_ONLY", "congestion_control": "cubic",
            "udp_relay_mode": "", "udp_over_stream": false,
            "zero_rtt_handshake": false, "heartbeat": "10s",
        }),
        "anytls" => json!({
            "password": "TEST_ONLY", "idle_session_check_interval": "30s",
            "idle_session_timeout": "30s", "min_idle_session": 0,
        }),
        "socks" => json!({"version": "5"}),
        "http" => json!({"path": "", "headers": {}}),
        _ => unreachable!("test protocol"),
    };
    value
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    if matches!(
        kind,
        "vmess" | "vless" | "trojan" | "hysteria2" | "tuic" | "anytls"
    ) {
        value["tls"] = json!({"enabled": true});
    }
    value
}

fn normalized_node(id: i64, name: &str, value: Value) -> ExternalClientNode {
    let normalized: NormalizedOutbound = serde_json::from_value(value).unwrap();
    ExternalClientNode {
        id,
        name: name.into(),
        sort_order: id,
        outbound: ExternalOutbound::from_normalized(&normalized).unwrap(),
    }
}

fn h2(kind: &str, method: &str, headers: Value) -> Value {
    let mut value = defaults(kind);
    value["transport"] = json!({
        "type": "http", "host": ["h2.example.invalid"], "path": "/h2",
        "method": method, "headers": headers,
    });
    value
}

#[test]
fn normalized_defaults_preserve_all_nine_supported_protocols() {
    let protocols = [
        ("shadowsocks", "ss"),
        ("vmess", "vmess"),
        ("trojan", "trojan"),
        ("vless", "vless"),
        ("hysteria2", "hysteria2"),
        ("tuic", "tuic"),
        ("anytls", "anytls"),
        ("socks", "socks5"),
        ("http", "http"),
    ];
    let nodes: Vec<_> = protocols
        .iter()
        .enumerate()
        .map(|(index, (kind, _))| {
            let node = normalized_node(index as i64 + 1, kind, defaults(kind));
            match *kind {
                "http" => {
                    assert_eq!(node.outbound.0["path"], "");
                    assert_eq!(node.outbound.0["headers"], json!({}));
                }
                "hysteria2" => {
                    for key in ["server_ports", "bbr_profile", "disable_chrome_parrot"] {
                        assert!(node.outbound.0.get(key).is_none(), "{key}");
                    }
                }
                "tuic" => assert!(node.outbound.0.get("udp_relay_mode").is_none()),
                _ => {}
            }
            node
        })
        .collect();
    let profile = compile_clash(&[], 7, &nodes).unwrap();
    assert_eq!(profile.included, protocols.len(), "{:?}", profile.skipped);
    assert!(profile.skipped.is_empty());
    for (kind, target) in protocols {
        assert!(
            profile
                .content
                .contains(&format!("  - name: \"{kind}\"\n    type: \"{target}\"\n"))
        );
    }
}

#[test]
fn normalized_http_empty_path_keeps_request_headers_and_authentication() {
    let mut value = defaults("http");
    value["username"] = json!("test-user");
    value["password"] = json!("TEST_ONLY_PASSWORD");
    value["headers"] = json!({"x-test": ["TEST_ONLY_HEADER"]});
    let node = normalized_node(1, "HTTP", value);
    let profile = compile_clash(&[], 7, &[node]).unwrap();
    assert_eq!(profile.included, 1, "{:?}", profile.skipped);
    assert!(profile.skipped.is_empty());
    for expected in [
        "username: \"test-user\"\n",
        "password: \"TEST_ONLY_PASSWORD\"\n",
        "x-test: \"TEST_ONLY_HEADER\"\n",
    ] {
        assert!(profile.content.contains(expected), "{expected}");
    }
    assert!(!profile.content.contains("path:"));
}

#[test]
fn normalized_http_custom_paths_do_not_remove_compatible_nodes() {
    for path in ["/", "/connect", " "] {
        let mut value = defaults("http");
        value["path"] = json!(path);
        let nodes = [
            normalized_node(1, "Working", defaults("http")),
            normalized_node(2, "Custom path", value),
        ];
        let profile = compile_clash(&[], 7, &nodes).unwrap();
        assert_eq!(profile.included, 1, "{path}: {:?}", profile.skipped);
        assert_eq!(profile.skipped.len(), 1, "{path}");
        assert_eq!(profile.skipped[0].0, "Custom path");
        assert!(profile.content.contains("  - name: \"Working\"\n"));
        assert!(!profile.content.contains("  - name: \"Custom path\"\n"));
    }
}

#[test]
fn normalized_h2_equivalent_methods_keep_hosts_paths_and_tls() {
    let mut nodes = vec![normalized_node(1, "HTTP", defaults("http"))];
    for kind in ["vmess", "vless"] {
        for method in ["", "PUT"] {
            let id = nodes.len() as i64 + 1;
            let name = format!("{kind} h2 {id}");
            let node = normalized_node(id, &name, h2(kind, method, json!({})));
            assert_eq!(node.outbound.0["transport"]["method"], method);
            assert_eq!(node.outbound.0["transport"]["headers"], json!({}));
            nodes.push(node);
        }
    }
    let profile = compile_clash(&[], 7, &nodes).unwrap();
    assert_eq!(profile.included, 5, "{:?}", profile.skipped);
    assert!(profile.skipped.is_empty());
    for node in &nodes[1..] {
        assert!(
            profile
                .content
                .contains(&format!("  - name: \"{}\"\n", node.name))
        );
    }
    for expected in [
        "network: \"h2\"\n",
        "path: \"/h2\"\n",
        "- \"h2.example.invalid\"\n",
        "tls: true\n",
    ] {
        assert_eq!(profile.content.matches(expected).count(), 4, "{expected}");
    }
    assert!(!profile.content.contains("method:"));
    // A real client can validate this profile without connecting to the nodes.
    if let Ok(path) = std::env::var("APSP_CLASH_NORMALIZED_SAMPLE") {
        std::fs::write(path, &profile.content).unwrap();
    }
}

#[test]
fn normalized_h2_custom_methods_do_not_remove_compatible_nodes() {
    for kind in ["vmess", "vless"] {
        for method in ["GET", "POST", "HEAD", "put"] {
            let nodes = [
                normalized_node(1, "Working", defaults("http")),
                normalized_node(2, "Custom method", h2(kind, method, json!({}))),
            ];
            let profile = compile_clash(&[], 7, &nodes).unwrap();
            assert_eq!(
                profile.included, 1,
                "{kind} {method}: {:?}",
                profile.skipped
            );
            assert_eq!(profile.skipped.len(), 1, "{kind} {method}");
            assert_eq!(profile.skipped[0].0, "Custom method");
            assert!(profile.content.contains("  - name: \"Working\"\n"));
            assert!(!profile.content.contains("network: \"h2\"\n"));
        }
    }
}

#[test]
fn normalized_h2_request_headers_cannot_be_silently_discarded() {
    for kind in ["vmess", "vless"] {
        for headers in [
            json!({"x-test": ["TEST_ONLY_HEADER"]}),
            json!({"host": ["other.example.invalid"]}),
        ] {
            let nodes = [
                normalized_node(1, "Working", defaults("http")),
                normalized_node(2, "Custom headers", h2(kind, "PUT", headers)),
            ];
            let profile = compile_clash(&[], 7, &nodes).unwrap();
            assert_eq!(profile.included, 1, "{kind}: {:?}", profile.skipped);
            assert_eq!(profile.skipped.len(), 1, "{kind}");
            assert_eq!(profile.skipped[0].0, "Custom headers");
            assert!(profile.content.contains("  - name: \"Working\"\n"));
            assert!(!profile.content.contains("network: \"h2\"\n"));
        }
    }
}

#[test]
fn normalized_h2_keeps_tls_and_trojan_compatibility_boundaries() {
    for kind in ["vmess", "vless", "trojan"] {
        for enabled in [None, Some(false), Some(true)] {
            if kind != "trojan" && enabled == Some(true) {
                continue;
            }
            let mut value = h2(kind, "PUT", json!({}));
            if let Some(enabled) = enabled {
                value["tls"] = json!({"enabled": enabled});
            } else {
                value.as_object_mut().unwrap().remove("tls");
            }
            let node = normalized_node(1, "Incompatible h2", value);
            let profile = compile_clash(&[], 7, &[node]).unwrap();
            assert_eq!(profile.included, 0, "{kind}: {enabled:?}");
            assert_eq!(profile.skipped.len(), 1, "{kind}: {enabled:?}");
            assert!(!profile.content.contains("network: \"h2\"\n"));
        }
    }
}

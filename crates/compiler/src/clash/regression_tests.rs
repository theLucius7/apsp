use super::*;
use crate::external::{ExternalOutbound, NormalizedOutbound};

fn external(id: i64, name: &str, outbound: Value) -> ExternalClientNode {
    ExternalClientNode {
        id,
        name: name.into(),
        sort_order: id,
        outbound: ExternalOutbound(outbound),
    }
}

fn vmess(transport: Value) -> Value {
    json!({
        "type": "vmess", "server": "vmess.example.invalid", "server_port": 443,
        "uuid": "00000000-0000-4000-8000-000000000001", "security": "auto",
        "alter_id": 0, "global_padding": false, "authenticated_length": false,
        "tls": {"enabled": true, "server_name": "vmess.example.invalid"},
        "transport": transport,
    })
}

fn http() -> Value {
    json!({"type": "http", "server": "http.example.invalid", "server_port": 8080})
}

fn with_headers(kind: &str, value: Value) -> Value {
    let headers = json!({"X-Single": value, "X-Text": "TEST_ONLY_TEXT"});
    match kind {
        "http" => {
            let mut outbound = http();
            outbound["headers"] = headers;
            outbound
        }
        "httpupgrade" => vmess(json!({
            "type": "httpupgrade", "path": "/upgrade",
            "host": "upgrade.example.invalid", "headers": headers,
        })),
        "ws" => vmess(json!({"type": "ws", "path": "/ws", "headers": headers})),
        _ => unreachable!("test transport"),
    }
}

#[test]
fn normalized_external_websocket_headers_compile_as_strings() {
    let normalized: NormalizedOutbound = serde_json::from_value(vmess(json!({
        "type": "ws", "path": "/ws", "headers": {"Host": ["ws.example.invalid"]},
        "max_early_data": 0, "early_data_header_name": "",
    })))
    .unwrap();
    let outbound = ExternalOutbound::from_normalized(&normalized).unwrap();
    let node = ExternalClientNode {
        id: 1,
        name: "Normalized WebSocket".into(),
        sort_order: 1,
        outbound,
    };
    let profile = compile_clash(&[], 7, &[node]).unwrap();
    assert_eq!(profile.included, 1);
    assert!(profile.skipped.is_empty());
    assert!(profile.content.contains("Host: \"ws.example.invalid\"\n"));
    assert!(!profile.content.contains("Host:\n"));
}

#[test]
fn external_request_headers_keep_strings_and_single_element_arrays() {
    for kind in ["ws", "httpupgrade", "http"] {
        for value in [json!("TEST_ONLY_SINGLE"), json!(["TEST_ONLY_SINGLE"])] {
            let node = external(1, kind, with_headers(kind, value));
            let profile = compile_clash(&[], 7, &[node]).unwrap();
            assert_eq!(profile.included, 1, "{kind}: {:?}", profile.skipped);
            assert!(profile.skipped.is_empty());
            assert!(profile.content.contains("X-Single: \"TEST_ONLY_SINGLE\"\n"));
            assert!(profile.content.contains("X-Text: \"TEST_ONLY_TEXT\"\n"));
        }
    }
}

#[test]
fn external_request_headers_skip_unrepresentable_nodes_without_breaking_others() {
    for kind in ["ws", "httpupgrade", "http"] {
        for value in [json!([]), json!(["first", "second"])] {
            let nodes = [
                external(1, "Working", http()),
                external(2, "Unrepresentable", with_headers(kind, value)),
            ];
            let profile = compile_clash(&[], 7, &nodes).unwrap();
            assert_eq!(profile.included, 1, "{kind}");
            assert_eq!(
                profile.skipped,
                vec![(
                    "Unrepresentable".into(),
                    "Clash 不能等价表达多值或空 HTTP 请求头"
                )],
                "{kind}",
            );
            assert!(profile.content.contains("  - name: \"Working\"\n"));
            assert!(!profile.content.contains("  - name: \"Unrepresentable\"\n"));
        }
    }
}

#[test]
fn external_httpupgrade_host_uses_the_same_single_value_boundary() {
    for host in [
        json!("upgrade.example.invalid"),
        json!(["upgrade.example.invalid"]),
    ] {
        let node = external(
            1,
            "Upgrade",
            vmess(json!({"type": "httpupgrade", "path": "/upgrade", "host": host})),
        );
        let profile = compile_clash(&[], 7, &[node]).unwrap();
        assert_eq!(profile.included, 1);
        assert!(
            profile
                .content
                .contains("Host: \"upgrade.example.invalid\"\n")
        );
    }
    for host in [
        json!([]),
        json!(["first.example.invalid", "second.example.invalid"]),
    ] {
        let node = external(
            1,
            "Upgrade",
            vmess(json!({"type": "httpupgrade", "path": "/upgrade", "host": host})),
        );
        let profile = compile_clash(&[], 7, &[node]).unwrap();
        assert_eq!(profile.included, 0);
        assert_eq!(
            profile.skipped[0].1,
            "Clash 不能等价表达多值或空 HTTP 请求头"
        );
    }
}

#[test]
fn external_trojan_http2_is_skipped_while_supported_transports_remain() {
    let trojan = |transport| {
        json!({
            "type": "trojan", "server": "trojan.example.invalid", "server_port": 443,
            "password": "TEST_ONLY", "tls": {"enabled": true}, "transport": transport,
        })
    };
    let h2 = json!({"type": "http", "host": ["h2.example.invalid"], "path": "/h2"});
    let nodes = [
        external(1, "Trojan HTTP2", trojan(h2.clone())),
        external(2, "Trojan WS", trojan(json!({"type": "ws", "path": "/ws"}))),
        external(
            3,
            "Trojan gRPC",
            trojan(json!({"type": "grpc", "service_name": "svc"})),
        ),
        external(4, "VMess HTTP2", vmess(h2)),
    ];
    let profile = compile_clash(&[], 7, &nodes).unwrap();
    assert_eq!(profile.included, 3);
    assert_eq!(
        profile.skipped,
        vec![("Trojan HTTP2".into(), "Clash 的 Trojan 不支持 HTTP/2 传输")],
    );
    for name in ["Trojan WS", "Trojan gRPC", "VMess HTTP2"] {
        assert!(profile.content.contains(&format!("  - name: \"{name}\"\n")));
    }
    assert!(!profile.content.contains("  - name: \"Trojan HTTP2\"\n"));
    assert!(profile.content.contains("network: \"h2\"\n"));
}

#[test]
fn external_http_transport_requires_tls_before_mapping_to_http2() {
    for kind in ["vmess", "vless"] {
        for enabled in [None, Some(false), Some(true)] {
            let mut outbound = vmess(json!({
                "type": "http", "host": ["h2.example.invalid"], "path": "/h2",
            }));
            outbound["type"] = json!(kind);
            let fields = outbound.as_object_mut().unwrap();
            if kind == "vless" {
                for key in [
                    "security",
                    "alter_id",
                    "global_padding",
                    "authenticated_length",
                ] {
                    fields.remove(key);
                }
            }
            if let Some(enabled) = enabled {
                fields.insert("tls".into(), json!({"enabled": enabled}));
            } else {
                fields.remove("tls");
            }
            let node = external(1, "HTTP transport", outbound);
            let profile = compile_clash(&[], 7, &[node]).unwrap();
            if enabled == Some(true) {
                assert_eq!(profile.included, 1, "{kind}");
                assert!(profile.skipped.is_empty());
                assert!(profile.content.contains("network: \"h2\"\n"));
            } else {
                assert_eq!(profile.included, 0, "{kind}: {enabled:?}");
                assert_eq!(
                    profile.skipped,
                    vec![(
                        "HTTP transport".into(),
                        "Clash 不能将未启用 TLS 的 HTTP 传输等价转换为 HTTP/2"
                    )],
                );
                assert!(!profile.content.contains("network: \"h2\"\n"));
            }
        }
    }
}

#[test]
fn skipped_external_names_cannot_escape_yaml_comments_with_unicode_breaks() {
    let name = "Skipped\u{2028}external-controller: 127.0.0.1:19090\u{2029}#\u{feff}";
    let nodes = [
        external(1, "Working", http()),
        external(
            2,
            name,
            json!({
                "type": "socks", "version": "4", "server": "socks.example.invalid", "server_port": 1080,
            }),
        ),
    ];
    let profile = compile_clash(&[], 7, &nodes).unwrap();
    assert_eq!(profile.included, 1);
    assert_eq!(profile.skipped, vec![(name.into(), "Clash 只支持 SOCKS5")]);
    assert!(
        !profile
            .content
            .contains(['\u{2028}', '\u{2029}', '\u{feff}'])
    );
    let comment = profile
        .content
        .lines()
        .find(|line| line.contains("external-controller:"))
        .unwrap();
    assert!(comment.starts_with("#   Skipped "));
    assert!(comment.ends_with("：Clash 只支持 SOCKS5"));
    assert!(!profile.content.contains("\nexternal-controller:"));
}

#[test]
fn external_names_avoid_every_builtin_and_duplicate_generated_suffixes() {
    for name in [
        "节点选择",
        "自动选择",
        "DIRECT",
        "REJECT",
        "REJECT-DROP",
        "COMPATIBLE",
        "PASS",
        "PASS-RULE",
        "GLOBAL",
    ] {
        let occupied = format!("{name} (2)");
        let nodes = [
            external(1, &occupied, http()),
            external(2, name, http()),
            external(3, name, http()),
        ];
        let profile = compile_clash(&[], 7, &nodes).unwrap();
        assert_eq!(profile.included, 3);
        assert!(profile.skipped.is_empty());
        for suffix in [2, 3, 4] {
            assert!(
                profile
                    .content
                    .contains(&format!("  - name: \"{name} ({suffix})\"\n"))
            );
            assert!(
                profile
                    .content
                    .contains(&format!("      - \"{name} ({suffix})\"\n"))
            );
        }
        assert!(
            !profile
                .content
                .contains(&format!("  - name: \"{name}\"\n    type: \"http\""))
        );
    }
}

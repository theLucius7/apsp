use super::parse::{MAX_BODY, MAX_DEPTH, MAX_NODES, MAX_SCALAR, digest, parse};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;

#[test]
fn generated_reality_transport_links_roundtrip_without_dropping_parameters() {
    let (private_key, public_key) = super::super::business::generate_reality_keypair();
    let identity = uuid::Uuid::new_v4();
    for transport in [
        json!({"type":"ws","path":"/edge+a&b","host":"edge.example.com","max_early_data":2048,"early_data_header_name":"Sec-WebSocket-Protocol"}),
        json!({"type":"httpupgrade","path":"/upgrade+a&b","host":"edge.example.com"}),
        json!({"type":"grpc","service_name":"test.service_v1-api"}),
    ] {
        let node: apsp_compiler::Node = serde_json::from_value(json!({
            "id":1,"name":"测试 + 节点","port":443,"public_host":"proxy.example.com","sni":"www.example.com",
            "private_key":private_key,"public_key":public_key,"short_id":"abcd1234",
            "settings":{"reality":{"flow":"none"},"transport":transport},
            "users":[{"user_id":1,"uuid":identity}]
        })).unwrap();
        let client: serde_json::Value = serde_json::from_str(
            &apsp_compiler::compile_client(std::slice::from_ref(&node), 1).unwrap(),
        )
        .unwrap();
        let links = apsp_compiler::subscription_links(&[node], 1).unwrap();
        let parsed = parse(links.as_bytes()).unwrap();
        assert!(parsed.rejected.is_empty());
        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.nodes[0].name, "测试 + 节点");
        assert_eq!(
            parsed.nodes[0].outbound.0["transport"],
            client["outbounds"][1]["transport"]
        );
        assert_eq!(
            parsed.nodes[0].outbound.0["tls"],
            client["outbounds"][1]["tls"]
        );
        assert_eq!(parsed.nodes[0].outbound.0["uuid"], identity.to_string());
    }
}

#[test]
fn websocket_early_data_uri_fields_validate_and_vmess_preserves_them() {
    let identity = uuid::Uuid::new_v4();
    for query in [
        "type=ws&ed=65536",
        "type=ws&ed=-1",
        "type=ws&ed=2.5",
        "type=ws&ed=1024&eh=bad%20header",
        "type=ws&ed=1024&eh=Host",
        "type=ws&ed=0&eh=X-Early",
        "type=grpc&ed=1024",
        "type=tcp&eh=X-Early",
    ] {
        let uri = format!("vless://{identity}@proxy.example.com:443?{query}");
        let parsed = parse(uri.as_bytes()).unwrap();
        assert!(parsed.nodes.is_empty(), "{query}");
        assert_eq!(parsed.rejected.len(), 1, "{query}");
    }
    let config = json!({"v":"2","ps":"WebSocket","add":"proxy.example.com","port":443,"id":identity,
        "aid":0,"net":"ws","tls":"tls","path":"/ws","ed":2048,"eh":"Sec-WebSocket-Protocol"});
    let uri = format!("vmess://{}", STANDARD.encode(config.to_string()));
    let parsed = parse(uri.as_bytes()).unwrap();
    assert!(parsed.rejected.is_empty());
    assert_eq!(
        parsed.nodes[0].outbound.0["transport"]["max_early_data"],
        2048
    );
    assert_eq!(
        parsed.nodes[0].outbound.0["transport"]["early_data_header_name"],
        "Sec-WebSocket-Protocol"
    );
}

#[test]
fn four_subscription_formats_produce_equivalent_proxy_parameters() {
    let uri = format!(
        "ss://{}@proxy.example.com:443#测试节点",
        STANDARD.encode("aes-128-gcm:fixture-password")
    );
    let json = json!({"outbounds":[{"type":"shadowsocks","tag":"测试节点","server":"proxy.example.com","server_port":443,"method":"aes-128-gcm","password":"fixture-password"}]}).to_string();
    let yaml = "proxies:\n  - name: 测试节点\n    type: ss\n    server: proxy.example.com\n    port: 443\n    cipher: aes-128-gcm\n    password: fixture-password\n";
    let encoded = STANDARD.encode(&uri);
    let results = [&uri, &encoded, &json, yaml].map(|text| parse(text.as_bytes()).unwrap());
    for result in &results {
        assert!(result.rejected.is_empty());
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].outbound, results[0].nodes[0].outbound);
        assert_eq!(
            result.nodes[0].identity_key,
            results[0].nodes[0].identity_key
        );
    }
}

#[test]
fn credentials_and_names_are_versions_not_identity() {
    let first = parse(b"proxies: [{name: first, type: ss, server: proxy.example.com, port: 443, cipher: aes-128-gcm, password: first-credential}]").unwrap();
    let second = parse(b"proxies: [{name: renamed, type: ss, server: proxy.example.com, port: 443, cipher: aes-128-gcm, password: rotated-credential}]").unwrap();
    assert_eq!(first.nodes[0].identity_key, second.nodes[0].identity_key);
    assert_ne!(first.nodes[0].outbound, second.nodes[0].outbound);
    let ambiguous = parse(b"proxies: [{name: first, type: ss, server: proxy.example.com, port: 443, cipher: aes-128-gcm, password: first-account}, {name: second, type: ss, server: proxy.example.com, port: 443, cipher: aes-128-gcm, password: second-account}]").unwrap();
    assert!(ambiguous.nodes.is_empty());
    assert_eq!(
        ambiguous.ambiguous_keys,
        vec![first.nodes[0].identity_key.clone()]
    );
    assert_eq!(ambiguous.rejected.len(), 2);
    let independent = parse(b"payload: [{provider_id: one, name: identical, type: http, server: proxy.example.com, port: 443}, {provider_id: two, name: identical, type: http, server: proxy.example.com, port: 443}]").unwrap();
    assert_eq!(independent.nodes.len(), 2);
    assert_ne!(
        independent.nodes[0].identity_key,
        independent.nodes[1].identity_key
    );
}

#[test]
fn renaming_and_reordering_keep_identity_and_semantic_digest() {
    let first = parse(b"proxies: [{name: first, type: http, server: PROXY.EXAMPLE.COM, port: 443, username: fixture, password: credential}, {name: second, type: socks5, server: proxy.example.com, port: 1080}]").unwrap();
    let reordered = parse(b"proxies: [{name: renamed-second, type: socks5, server: proxy.example.com, port: 1080}, {name: renamed-first, type: http, server: proxy.example.com, port: 443, username: fixture, password: credential}]").unwrap();
    for (original, renamed) in first.nodes.iter().zip(reordered.nodes.iter().rev()) {
        assert_ne!(original.name, renamed.name);
        assert_eq!(original.identity_key, renamed.identity_key);
        assert_eq!(original.outbound, renamed.outbound);
        assert_eq!(
            digest(&serde_json::to_vec(&original.outbound).unwrap()),
            digest(&serde_json::to_vec(&renamed.outbound).unwrap())
        );
    }
}

#[test]
fn rejects_duplicate_keys_depth_recursive_aliases_and_expansion() {
    assert!(parse(br#"{"outbounds":[],"outbounds":[]}"#).is_err());
    assert!(parse(b"proxies: []\nproxies: []").is_err());
    let deep = format!(
        "proxies: {}0{}",
        "[".repeat(MAX_DEPTH + 1),
        "]".repeat(MAX_DEPTH + 1)
    );
    assert_eq!(parse(deep.as_bytes()).err().unwrap().0, "structure_limit");
    assert_eq!(
        parse(b"proxies: &recursive [*recursive]").err().unwrap().0,
        "recursive_or_unknown_yaml_alias"
    );
    let mut bomb = String::from("a: &a [value, value, value, value, value, value, value, value]\n");
    let mut previous = "a".to_string();
    for i in 0..10 {
        let name = format!("b{i}");
        bomb.push_str(&format!(
            "{name}: &{name} [{}]\n",
            vec![format!("*{previous}"); 8].join(", ")
        ));
        previous = name;
    }
    bomb.push_str("proxies: []");
    assert_eq!(parse(bomb.as_bytes()).err().unwrap().0, "structure_limit");
    assert_eq!(
        parse(&vec![b'x'; MAX_BODY + 1]).err().unwrap().0,
        "body_limit"
    );
    let scalar = format!("ignored: '{}'\nproxies: []", "x".repeat(MAX_SCALAR + 1));
    assert_eq!(parse(scalar.as_bytes()).err().unwrap().0, "scalar_limit");
    let values = format!("ignored: [{}]\nproxies: []", "0,".repeat(100_001));
    assert_eq!(parse(values.as_bytes()).err().unwrap().0, "structure_limit");
    let json_depth = format!(
        "{{\"outbounds\":[],\"ignored\":{}0{}}}",
        "[".repeat(MAX_DEPTH + 1),
        "]".repeat(MAX_DEPTH + 1)
    );
    assert_eq!(
        parse(json_depth.as_bytes()).err().unwrap().0,
        "invalid_json_or_limit"
    );
    assert!(
        parse(
            "http://proxy.example.com:443\n"
                .repeat(MAX_NODES + 1)
                .as_bytes()
        )
        .is_err()
    );
}

#[test]
fn bounded_aliases_and_merges_work_without_accepting_duplicate_explicit_fields() {
    let value = parse(b"base: &base {type: http, server: proxy.example.com, port: 443}\nproxies:\n  - <<: *base\n    name: fixture\n").unwrap();
    assert_eq!(value.nodes.len(), 1);
    assert_eq!(value.nodes[0].outbound.server(), "proxy.example.com");
    assert!(
        parse(b"proxies: [{name: x, name: y, type: http, server: proxy.example.com, port: 443}]")
            .is_err()
    );
}

#[test]
fn reports_unsupported_semantics_without_exporting_secrets() {
    let value = parse(br#"{"outbounds":[{"type":"http","tag":"fixture","server":"proxy.example.com","server_port":443,"detour":"SECRET_TOKEN"}],"route":{"final":"do-not-import"}}"#).unwrap();
    assert!(value.nodes.is_empty());
    let public = serde_json::to_string(&value.rejected).unwrap();
    assert!(!public.contains("SECRET_TOKEN"));
    assert!(value.rejected[0].reason.contains("unsupported"));
    let value = parse(b"payload: [{name: fixture, type: ss, server: proxy.example.com, port: 443, cipher: aes-128-gcm, password: hidden, plugin: obfs, plugin-opts: {mode: tls}}]").unwrap();
    assert_eq!(value.rejected.len(), 1);
    assert!(
        parse(b"proxy-providers: {provider: {url: https://provider.example.com/private}} ")
            .is_err()
    );
}

#[test]
fn valid_proxies_never_import_source_routes_dns_providers_scripts_or_controls() {
    let json = br#"{
        "outbounds":[{"type":"http","tag":"source-tag","server":"proxy.example.com","server_port":443,"username":"fixture","password":"fixture-secret"}],
        "inbounds":[{"type":"mixed","listen":"0.0.0.0","listen_port":9999}],
        "route":{"final":"source-direct","rules":[{"action":"resolve","server":"source-dns"}]},
        "dns":{"servers":[{"type":"https","tag":"source-dns","server":"untrusted.example.com"}]},
        "experimental":{"clash_api":{"external_controller":"0.0.0.0:9998","secret":"source-control-secret"}},
        "script":"source-script-secret"
    }"#;
    let yaml = b"proxies:\n  - name: source-tag\n    type: http\n    server: proxy.example.com\n    port: 443\n    username: fixture\n    password: fixture-secret\nproxy-providers: {other: {type: http, url: https://provider.example.com/secret}}\nproxy-groups: [{name: source-group, type: select, proxies: [source-tag]}]\nrules: [MATCH,source-direct]\ndns: {enable: true, nameserver: [https://untrusted.example.com/dns-query]}\nscript: {code: source-script-secret}\nexternal-controller: 0.0.0.0:9998\nsecret: source-control-secret\n";
    let expected = json!({"type":"http","server":"proxy.example.com","server_port":443,"username":"fixture","password":"fixture-secret"});
    for bytes in [json.as_slice(), yaml.as_slice()] {
        let parsed = parse(bytes).unwrap();
        assert!(parsed.rejected.is_empty());
        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.nodes[0].outbound.0, expected);
        let rendered = parsed.nodes[0]
            .outbound
            .render(
                "compiled-tag",
                Some("compiled-previous"),
                Some("compiled-resolver"),
            )
            .unwrap();
        assert_eq!(rendered["tag"], "compiled-tag");
        assert_eq!(rendered["detour"], "compiled-previous");
        let output = rendered.to_string();
        for forbidden in [
            "source-tag",
            "source-direct",
            "source-dns",
            "source-control-secret",
            "source-script-secret",
            "provider.example.com",
            "untrusted.example.com",
            "0.0.0.0",
        ] {
            assert!(!output.contains(forbidden), "{forbidden}");
        }
    }
}

#[test]
fn sip002_distinguishes_ss2022_and_ipv6_percent_encoded_passwords() {
    let password = STANDARD.encode([1u8; 16]);
    let uri = format!("ss://2022-blake3-aes-128-gcm:{password}@%5B2001:db8::1%5D:443");
    let invalid = parse(uri.as_bytes()).unwrap();
    assert_eq!(invalid.rejected.len(), 1);
    let uri = format!("ss://2022-blake3-aes-128-gcm:{password}@[2001:db8::1]:443#fixture");
    let valid = parse(uri.as_bytes()).unwrap();
    assert_eq!(valid.nodes.len(), 1);
    assert_eq!(valid.nodes[0].outbound.server(), "2001:db8::1");
    let encoded = format!(
        "ss://{}@proxy.example.com:443",
        STANDARD.encode(format!("2022-blake3-aes-128-gcm:{password}"))
    );
    assert_eq!(
        parse(encoded.as_bytes()).unwrap().rejected[0].reason,
        "ss2022_requires_plain_userinfo"
    );
    let legacy = format!(
        "ss://{}",
        STANDARD.encode(format!(
            "2022-blake3-aes-128-gcm:{password}@proxy.example.com:443"
        ))
    );
    assert_eq!(
        parse(legacy.as_bytes()).unwrap().rejected[0].reason,
        "ss2022_requires_plain_userinfo"
    );
}

#[test]
fn mihomo_http_transports_reject_tls_combinations_that_change_the_wire_protocol() {
    for (network, tls, reason) in [
        ("h2", None, "unsupported_h2_without_tls"),
        ("h2", Some(false), "unsupported_h2_without_tls"),
        ("http", Some(true), "unsupported_http_with_tls"),
    ] {
        let mut node = json!({
            "name":"transport fixture","type":"vmess","server":"proxy.example.com",
            "port":443,"uuid":"00000000-0000-0000-0000-000000000001",
            "cipher":"auto","network":network
        });
        if let Some(tls) = tls {
            node["tls"] = json!(tls);
        }
        let parsed = parse(json!({"proxies":[node]}).to_string().as_bytes()).unwrap();
        assert!(parsed.nodes.is_empty());
        assert_eq!(parsed.rejected.len(), 1);
        assert_eq!(parsed.rejected[0].reason, reason);
    }
}

#[test]
fn mihomo_equivalent_http_transports_preserve_the_source_method_and_tls() {
    for (network, tls, method, expected_method) in [
        ("http", false, None, Some("GET")),
        ("http", false, Some(""), Some("GET")),
        ("http", false, Some("POST"), Some("POST")),
        ("h2", true, None, None),
    ] {
        let mut options = json!({"path":"/fixture"});
        if let Some(method) = method {
            options["method"] = json!(method);
        }
        let mut node = json!({
            "type":"vmess","server":"proxy.example.com","port":443,
            "uuid":"00000000-0000-0000-0000-000000000001",
            "cipher":"auto","network":network,"tls":tls
        });
        node[if network == "h2" {
            "h2-opts"
        } else {
            "http-opts"
        }] = options;
        let parsed = parse(json!({"payload":[node]}).to_string().as_bytes()).unwrap();
        assert!(parsed.rejected.is_empty());
        assert_eq!(parsed.nodes.len(), 1);
        let outbound = &parsed.nodes[0].outbound.0;
        assert_eq!(outbound.pointer("/transport/type"), Some(&json!("http")));
        assert_eq!(
            outbound.pointer("/transport/path"),
            Some(&json!("/fixture"))
        );
        assert_eq!(
            outbound
                .pointer("/transport/method")
                .and_then(serde_json::Value::as_str),
            expected_method
        );
        assert_eq!(
            outbound
                .pointer("/tls/enabled")
                .and_then(serde_json::Value::as_bool),
            tls.then_some(true)
        );
    }
}

#[test]
fn uri_and_base64_lists_do_not_silently_change_h2_to_plain_http_or_http_to_h2() {
    let uuid = "00000000-0000-0000-0000-000000000001";
    for (params, reason) in [
        ("type=h2", "unsupported_h2_without_tls"),
        ("type=h2&security=none", "unsupported_h2_without_tls"),
        ("type=http&security=tls", "unsupported_http_with_tls"),
    ] {
        let uri = format!("vless://{uuid}@proxy.example.com:443?{params}#fixture");
        for text in [uri.clone(), STANDARD.encode(&uri)] {
            let parsed = parse(text.as_bytes()).unwrap();
            assert!(parsed.nodes.is_empty());
            assert_eq!(parsed.rejected.len(), 1);
            assert_eq!(parsed.rejected[0].reason, reason);
        }
    }
    for (network, tls, reason) in [
        ("h2", "", "unsupported_h2_without_tls"),
        ("h2", "none", "unsupported_h2_without_tls"),
        ("http", "tls", "unsupported_http_with_tls"),
    ] {
        let config = json!({
            "v":"2","add":"proxy.example.com","port":"443","id":uuid,
            "aid":"0","net":network,"tls":tls,"type":"none"
        });
        let uri = format!("vmess://{}", STANDARD.encode(config.to_string()));
        let parsed = parse(uri.as_bytes()).unwrap();
        assert!(parsed.nodes.is_empty());
        assert_eq!(parsed.rejected[0].reason, reason);
    }
}

#[test]
fn equivalent_uri_transports_and_native_http_json_keep_their_declared_semantics() {
    let uuid = "00000000-0000-0000-0000-000000000001";
    for (params, tls) in [
        ("type=h2&security=tls", true),
        ("type=http&security=none", false),
    ] {
        let uri = format!("vless://{uuid}@proxy.example.com:443?{params}&path=%2Ffixture");
        let parsed = parse(uri.as_bytes()).unwrap();
        assert!(parsed.rejected.is_empty());
        assert_eq!(parsed.nodes.len(), 1);
        let outbound = &parsed.nodes[0].outbound.0;
        assert_eq!(outbound.pointer("/transport/type"), Some(&json!("http")));
        assert_eq!(
            outbound.pointer("/transport/path"),
            Some(&json!("/fixture"))
        );
        assert_eq!(
            outbound
                .pointer("/tls/enabled")
                .and_then(serde_json::Value::as_bool),
            tls.then_some(true)
        );
    }
    for tls in [false, true] {
        let node = json!({
            "type":"vmess","server":"proxy.example.com","server_port":443,
            "uuid":uuid,"security":"auto","tls":{"enabled":tls},
            "transport":{"type":"http","path":"/fixture"}
        });
        let parsed = parse(json!({"outbounds":[node.clone()]}).to_string().as_bytes()).unwrap();
        assert!(parsed.rejected.is_empty());
        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.nodes[0].outbound.0, node);
    }
}

#[test]
fn mihomo_http_host_headers_preserve_the_wire_host_and_single_header_values() {
    let node = json!({
        "type":"vmess","server":"proxy.example.com","port":443,
        "uuid":"00000000-0000-0000-0000-000000000001","cipher":"auto",
        "network":"http","http-opts":{
            "headers":{
                "Host":["cover.example.com","cover2.example.com"],
                "X-Fixture":["one-value"]
            },"path":["/fixture"]
        }
    });
    let parsed = parse(json!({"proxies":[node]}).to_string().as_bytes()).unwrap();
    assert!(parsed.rejected.is_empty());
    assert_eq!(parsed.nodes.len(), 1);
    let outbound = &parsed.nodes[0].outbound.0;
    assert_eq!(
        outbound.pointer("/transport/host"),
        Some(&json!(["cover.example.com", "cover2.example.com"]))
    );
    assert!(outbound.pointer("/transport/headers/Host").is_none());
    assert_eq!(
        outbound.pointer("/transport/headers/X-Fixture"),
        Some(&json!(["one-value"]))
    );
    assert_eq!(
        outbound.pointer("/transport/path"),
        Some(&json!("/fixture"))
    );
    assert_eq!(outbound.pointer("/transport/method"), Some(&json!("GET")));
}

#[test]
fn mihomo_http_rejects_random_headers_and_ambiguous_host_fields() {
    for (options, reason) in [
        (
            json!({"headers":{"X-Fixture":["secret-one","secret-two"]}}),
            "unsupported_random_http_headers",
        ),
        (
            json!({"headers":{"Host":["cover.example.com"],"host":["other.example.com"]}}),
            "duplicate_http_host_parameter",
        ),
        (
            json!({"host":["other.example.com"],"headers":{"Host":["cover.example.com"]}}),
            "duplicate_http_host_parameter",
        ),
        (
            json!({"headers":{"host":["cover.example.com"]}}),
            "unsupported_http_host_header",
        ),
    ] {
        let node = json!({
            "type":"vmess","server":"proxy.example.com","port":443,
            "uuid":"00000000-0000-0000-0000-000000000001","cipher":"auto",
            "network":"http","http-opts":options
        });
        let parsed = parse(json!({"proxies":[node]}).to_string().as_bytes()).unwrap();
        assert!(parsed.nodes.is_empty());
        assert_eq!(parsed.rejected[0].reason, reason);
        assert!(
            !serde_json::to_string(&parsed.rejected)
                .unwrap()
                .contains("secret-")
        );
    }
}

#[test]
fn mihomo_h2_forces_source_alpn_and_rejects_foreign_transport_options() {
    let mut node = json!({
        "type":"vmess","server":"proxy.example.com","port":443,
        "uuid":"00000000-0000-0000-0000-000000000001","cipher":"auto",
        "network":"h2","tls":true,"alpn":["http/1.1"],
        "h2-opts":{"host":["cover.example.com"],"path":"/fixture"}
    });
    let parsed = parse(json!({"proxies":[node.clone()]}).to_string().as_bytes()).unwrap();
    assert!(parsed.rejected.is_empty());
    assert_eq!(parsed.nodes.len(), 1);
    assert_eq!(
        parsed.nodes[0].outbound.0.pointer("/tls/alpn"),
        Some(&json!(["h2"]))
    );
    for (key, value) in [
        ("headers", json!({"X-Fixture":"secret-value"})),
        ("method", json!("POST")),
    ] {
        node["h2-opts"][key] = value;
        let rejected = parse(json!({"proxies":[node.clone()]}).to_string().as_bytes()).unwrap();
        assert!(rejected.nodes.is_empty());
        assert_eq!(
            rejected.rejected[0].reason,
            "unsupported_transport_parameter"
        );
        node["h2-opts"].as_object_mut().unwrap().remove(key);
    }
    node["alpn"] = json!(42);
    let rejected = parse(json!({"proxies":[node]}).to_string().as_bytes()).unwrap();
    assert!(rejected.nodes.is_empty());
    assert_eq!(
        rejected.rejected[0].reason,
        "unsupported_or_invalid_proxy_parameter"
    );
}

use super::*;
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde_json::{Value, json};

const UUID: &str = "11111111-1111-4111-8111-111111111111";
const PASSWORD: &str = "TEST_ONLY-private-password";

fn parse(text: &str) -> ParsedSubscription {
    parse_subscription(text.as_bytes(), FormatHint::Auto).expect("valid fixture subscription")
}

fn json_source(nodes: Vec<Value>) -> String {
    json!({"outbounds":nodes}).to_string()
}

fn ss(name: &str, password: &str) -> Value {
    json!({"type":"shadowsocks","tag":name,"server":"edge.example.com","server_port":443,"method":"aes-128-gcm","password":password})
}

fn config(node: &ParsedNode) -> Value {
    serde_json::to_value(node.outbound.as_ref().expect("supported outbound"))
        .expect("serializable config")
}

fn failure(text: &str, hint: FormatHint) -> ParseError {
    parse_subscription(text.as_bytes(), hint)
        .err()
        .expect("rejected document")
}

#[test]
fn four_formats_have_one_connection_identity_and_version() {
    let userinfo = URL_SAFE_NO_PAD.encode(format!("aes-128-gcm:{PASSWORD}"));
    let uri = format!("ss://{userinfo}@edge.example.com:443#%E4%B8%AD%E6%96%87%E8%8A%82%E7%82%B9");
    let fixtures = [
        uri.clone(),
        STANDARD.encode(&uri),
        json_source(vec![ss("中文节点", PASSWORD)]),
        format!(
            "proxies:\n  - name: 中文节点\n    type: ss\n    server: edge.example.com\n    port: 443\n    cipher: aes-128-gcm\n    password: {PASSWORD}\n"
        ),
    ];
    let formats = [
        SubscriptionFormat::UriList,
        SubscriptionFormat::Base64UriList,
        SubscriptionFormat::SingBoxJson,
        SubscriptionFormat::ClashYaml,
    ];
    let batches = fixtures
        .iter()
        .map(|fixture| parse(fixture))
        .collect::<Vec<_>>();
    for (batch, format) in batches.iter().zip(formats) {
        assert_eq!(batch.format, format);
        assert_eq!(batch.supported_count, 1);
        assert_eq!(batch.unsupported_count, 0);
        assert_eq!(batch.nodes[0].preview.name, "中文节点");
        assert_eq!(
            batch.nodes[0].content_digest,
            batches[0].nodes[0].content_digest
        );
        assert_eq!(
            batch.nodes[0].identity_fingerprint,
            batches[0].nodes[0].identity_fingerprint
        );
        assert_eq!(config(&batch.nodes[0]), config(&batches[0].nodes[0]));
        assert!(batch.nodes[0].provider_metadata_id.is_none());
    }
    assert_ne!(batches[0].raw_digest, batches[1].raw_digest);
}

#[test]
fn credentials_names_order_and_ambiguous_accounts_do_not_become_identity() {
    let original = parse(&json_source(vec![ss("名称", PASSWORD)]));
    let rotated = parse(&json_source(vec![ss(
        "新名称",
        "TEST_ONLY-rotated-password",
    )]));
    assert_eq!(
        original.nodes[0].identity_fingerprint,
        rotated.nodes[0].identity_fingerprint
    );
    assert_ne!(
        original.nodes[0].content_digest,
        rotated.nodes[0].content_digest
    );
    let renamed = parse(&json_source(vec![ss("新名称", PASSWORD)]));
    assert_eq!(
        original.nodes[0].content_digest,
        renamed.nodes[0].content_digest
    );
    let mut different_endpoint = ss("名称", PASSWORD);
    different_endpoint["server"] = json!("other.example.com");
    let ordered = parse(&json_source(vec![
        ss("名称", PASSWORD),
        different_endpoint.clone(),
        ss("账号二", "TEST_ONLY-second-account"),
    ]));
    let reversed = parse(&json_source(vec![
        ss("账号二", "TEST_ONLY-second-account"),
        different_endpoint,
        ss("名称", PASSWORD),
    ]));
    assert_eq!(
        ordered.nodes[0].identity_fingerprint,
        ordered.nodes[2].identity_fingerprint
    );
    assert_ne!(
        ordered.nodes[0].identity_fingerprint,
        ordered.nodes[1].identity_fingerprint
    );
    assert_eq!(
        ordered.nodes[0].content_digest,
        reversed.nodes[2].content_digest
    );
    assert_eq!(
        ordered.nodes[1].content_digest,
        reversed.nodes[1].content_digest
    );
    // Batch persistence, rather than this parser, decides that duplicate fingerprints are ambiguous.
    assert_eq!(ordered.nodes.len(), 3);
}

#[test]
fn all_supported_protocols_preserve_their_semantic_parameters() {
    let public_key = URL_SAFE_NO_PAD.encode([9u8; 32]);
    let mut vmess = json!({"type":"vmess","server":"vm.example.com","server_port":443,"uuid":UUID,"security":"aes-128-gcm","alter_id":2,
        "global_padding":true,"authenticated_length":true,"packet_encoding":"packetaddr","tls":{"enabled":true,"server_name":"sni.example.com","alpn":["h2"],"utls":{"enabled":true,"fingerprint":"firefox"}},
        "transport":{"type":"ws","path":"/transport","headers":{"Host":"sni.example.com","Authorization":PASSWORD},"max_early_data":1024,"early_data_header_name":"Sec-WebSocket-Protocol"},
        "multiplex":{"enabled":true,"protocol":"yamux","max_connections":2,"min_streams":2,"max_streams":4,"padding":true}});
    vmess["tag"] = json!("VMess");
    let nodes = vec![
        ss("SS", PASSWORD),
        vmess,
        json!({"type":"trojan","server":"trojan.example.com","server_port":443,"password":PASSWORD,"tls":{"enabled":true,"server_name":"sni.example.com"},"transport":{"type":"grpc","service_name":"service","permit_without_stream":true}}),
        json!({"type":"vless","server":"vless.example.com","server_port":443,"uuid":UUID,"flow":"xtls-rprx-vision","packet_encoding":"xudp",
            "tls":{"enabled":true,"server_name":"sni.example.com","utls":{"enabled":true,"fingerprint":"chrome"},"reality":{"enabled":true,"public_key":public_key,"short_id":"01ab"}}}),
        json!({"type":"hysteria2","server":"hy.example.com","server_port":443,"password":PASSWORD,"server_ports":["443:445","8443"],"hop_interval":"30s","up_mbps":25,"down_mbps":50,"obfs":{"type":"salamander","password":"TEST_ONLY-obfs"},"tls":{"enabled":true,"server_name":"hy.example.com"}}),
        json!({"type":"tuic","server":"tuic.example.com","server_port":443,"uuid":UUID,"password":PASSWORD,"congestion_control":"bbr","udp_over_stream":true,"zero_rtt_handshake":true,"heartbeat":"12s","tls":{"enabled":true,"alpn":["h3"]}}),
        json!({"type":"anytls","server":"any.example.com","server_port":443,"password":PASSWORD,"idle_session_check_interval":"20s","idle_session_timeout":"60s","min_idle_session":3,"tls":{"enabled":true}}),
        json!({"type":"socks","server":"socks.example.com","server_port":1080,"version":"5","username":"TEST_ONLY-user","password":PASSWORD,"udp_over_tcp":{"enabled":true,"version":1}}),
        json!({"type":"http","server":"http.example.com","server_port":443,"username":"TEST_ONLY-user","password":PASSWORD,"tls":{"enabled":true,"server_name":"http.example.com"},"path":"/proxy","headers":{"Proxy-Authorization":PASSWORD}}),
    ];
    let batch = parse(&json_source(nodes));
    assert_eq!(batch.supported_count, 9);
    let vmess = config(&batch.nodes[1]);
    assert_eq!(vmess["alter_id"], 2);
    assert_eq!(
        vmess["transport"]["headers"]["authorization"],
        json!([PASSWORD])
    );
    assert_eq!(vmess["transport"]["max_early_data"], 1024);
    assert_eq!(vmess["multiplex"]["protocol"], "yamux");
    assert_eq!(
        config(&batch.nodes[2])["transport"]["service_name"],
        "service"
    );
    assert_eq!(
        config(&batch.nodes[3])["tls"]["reality"]["short_id"],
        "01ab"
    );
    assert_eq!(
        config(&batch.nodes[4])["server_ports"],
        json!(["443:445", "8443"])
    );
    assert_eq!(config(&batch.nodes[5])["udp_over_stream"], true);
    assert_eq!(config(&batch.nodes[6])["idle_session_timeout"], "60s");
    assert_eq!(config(&batch.nodes[7])["udp_over_tcp"]["version"], 1);
    assert_eq!(
        config(&batch.nodes[8])["headers"]["proxy-authorization"],
        json!([PASSWORD])
    );
    assert!(config(&batch.nodes[8]).get("network").is_none());
    assert!(config(&batch.nodes[6]).get("network").is_none());
    assert!(!batch.nodes[8].outbound.as_ref().expect("HTTP").udp());
    assert!(
        batch
            .nodes
            .iter()
            .all(|node| node.outbound.as_ref().expect("supported").tcp())
    );
    let preview = serde_json::to_string(
        &batch
            .nodes
            .iter()
            .map(|node| &node.preview)
            .collect::<Vec<_>>(),
    )
    .expect("public preview");
    assert!(!preview.contains(PASSWORD));
    assert!(!preview.contains(UUID));
    assert!(!preview.contains("/transport"));
    assert!(!preview.contains("01ab"));
    // After the source migration, ordered-source versions reach client
    // subscriptions through the numbered-source validator. Only options that
    // numbered sources never accepted (here Hysteria2 port hopping) are left out.
    let client = |outbound: &NormalizedOutbound| {
        apsp_compiler::external::ExternalOutbound::from_normalized(outbound).is_ok()
    };
    let rejected: Vec<_> = batch
        .nodes
        .iter()
        .map(|node| node.outbound.as_ref().expect("supported"))
        .filter(|outbound| !client(outbound))
        .map(NormalizedOutbound::protocol)
        .collect();
    assert_eq!(rejected, [ExternalProtocol::Hysteria2]);
    let defaults = parse(&json_source(vec![
        json!({"type":"hysteria2","server":"hy.example.com","server_port":443,"password":PASSWORD,"tls":{"enabled":true}}),
        json!({"type":"tuic","server":"tuic.example.com","server_port":443,"uuid":UUID,"password":PASSWORD,"tls":{"enabled":true}}),
    ]));
    assert_eq!(defaults.nodes.len(), 2);
    for node in &defaults.nodes {
        assert!(client(node.outbound.as_ref().expect("supported")));
    }
    let invalid = parse(&json_source(vec![
        json!({"type":"tuic","server":"tuic.example.com","server_port":443,"uuid":UUID,"password":PASSWORD,"udp_relay_mode":"quic","udp_over_stream":true,"tls":{"enabled":true}}),
        json!({"type":"vless","server":"v.example.com","server_port":443,"uuid":UUID,"tls":{"enabled":true,"reality":{"enabled":true,"public_key":URL_SAFE_NO_PAD.encode([9u8;32]),"short_id":"01ab"}}}),
        json!({"type":"trojan","server":"t.example.com","server_port":443,"password":PASSWORD,"tls":{"enabled":true,"ech":{"enabled":true,"config":["TEST_ONLY-config"]}}}),
    ]));
    assert_eq!(invalid.unsupported_count, 3);
    assert!(invalid.nodes.iter().all(|node| node.outbound.is_none()));
}

#[test]
fn uri_options_ipv6_vmess_legacy_and_modern_auth_are_not_discarded() {
    let public_key = URL_SAFE_NO_PAD.encode([9u8; 32]);
    let vmess = json!({"v":"2","ps":"VMess","add":"vm.example.com","port":"443","id":UUID,"aid":"2","scy":"aes-128-gcm","net":"ws","type":"none","host":"sni.example.com","path":"/ws","tls":"tls","sni":"sni.example.com","alpn":"h2","fp":"chrome"});
    let lines = [format!("vmess://{}",STANDARD.encode(vmess.to_string())),
        format!("vless://{UUID}@[2001:db8::1]:443?security=reality&sni=sni.example.com&fp=chrome&pbk={public_key}&sid=01ab&flow=xtls-rprx-vision#Reality"),
        format!("trojan://{PASSWORD}@trojan.example.com:443?type=grpc&serviceName=service&sni=sni.example.com"),
        "hy2://TEST_ONLY-user:TEST_ONLY-password@hy.example.com:443?sni=hy.example.com&obfs=salamander&obfs-password=TEST_ONLY-obfs&upmbps=20&downmbps=30".into(),
        format!("tuic://{UUID}:{PASSWORD}@tuic.example.com:443?congestion_control=bbr&udp_relay_mode=quic&heartbeat=12s"),
        format!("anytls://{PASSWORD}@any.example.com:443?min_idle_session=2&idle_session_timeout=60s"),
        format!("socks5://TEST_ONLY-user:{PASSWORD}@socks.example.com:1080?uot=1"),
        format!("https://TEST_ONLY-user:{PASSWORD}@http.example.com:443?sni=http.example.com")];
    let batch = parse(&lines.join("\n"));
    assert_eq!(batch.supported_count, 8);
    assert_eq!(config(&batch.nodes[0])["alter_id"], 2);
    assert_eq!(
        batch.nodes[1].preview.server.as_deref(),
        Some("2001:db8::1")
    );
    assert_eq!(config(&batch.nodes[1])["flow"], "xtls-rprx-vision");
    assert_eq!(
        config(&batch.nodes[3])["password"],
        "TEST_ONLY-user:TEST_ONLY-password"
    );
    assert_eq!(config(&batch.nodes[4])["congestion_control"], "bbr");
    assert_eq!(config(&batch.nodes[5])["min_idle_session"], 2);
    assert_eq!(config(&batch.nodes[6])["udp_over_tcp"]["enabled"], true);
    assert_eq!(config(&batch.nodes[7])["tls"]["enabled"], true);
}

#[test]
fn ss2022_plain_userinfo_and_legacy_ss_encodings_are_distinct() {
    let key = STANDARD.encode([1u8; 16]);
    let plain = format!(
        "ss://2022-blake3-aes-128-gcm:{}@ss.example.com:443#SS2022",
        key.replace('=', "%3D")
    );
    let encoded = format!(
        "ss://{}@ss.example.com:443",
        URL_SAFE_NO_PAD.encode(format!("2022-blake3-aes-128-gcm:{key}"))
    );
    let valid = parse(&plain);
    assert_eq!(valid.supported_count, 1);
    assert_eq!(config(&valid.nodes[0])["password"], key);
    let invalid = parse(&encoded);
    assert_eq!(invalid.unsupported_count, 1);
    assert_eq!(
        invalid.nodes[0].preview.unsupported_reasons[0].code,
        "invalid_credential"
    );
    let legacy = parse(&format!(
        "ss://{}#Old",
        STANDARD.encode(format!("aes-128-gcm:{PASSWORD}@ss.example.com:443"))
    ));
    assert_eq!(legacy.supported_count, 1);
    assert_eq!(config(&legacy.nodes[0])["password"], PASSWORD);
    let invalid_key = parse("ss://2022-blake3-aes-128-gcm:short@ss.example.com:443");
    assert_eq!(invalid_key.unsupported_count, 1);
    let uri_plugin = parse(&format!(
        "ss://{}@ss.example.com:443/?plugin=obfs-local%3Bobfs-host%3Dsni.example.com%3Bobfs%3Dhttp",
        URL_SAFE_NO_PAD.encode(format!("aes-128-gcm:{PASSWORD}"))
    ));
    let yaml_plugin = parse(&format!(
        "proxies:\n - type: ss\n   server: ss.example.com\n   port: 443\n   cipher: aes-128-gcm\n   password: {PASSWORD}\n   plugin: obfs\n   plugin-opts: {{mode: http, host: sni.example.com}}\n"
    ));
    assert_eq!(uri_plugin.supported_count, 1);
    assert_eq!(
        uri_plugin.nodes[0].content_digest,
        yaml_plugin.nodes[0].content_digest
    );
    let unknown_plugin = parse(&format!(
        "ss://{}@ss.example.com:443/?plugin=TEST_ONLY-plugin",
        URL_SAFE_NO_PAD.encode(format!("aes-128-gcm:{PASSWORD}"))
    ));
    assert_eq!(unknown_plugin.unsupported_count, 1);
}

#[test]
fn vless_implicit_xudp_differs_from_explicit_empty_packet_encoding() {
    let base = json!({"type":"vless","server":"vless.example.com","server_port":443,"uuid":UUID});
    let implicit = parse(&json_source(vec![base.clone()]));
    let mut explicit_xudp = base.clone();
    explicit_xudp["packet_encoding"] = json!("xudp");
    let mut explicit_empty = base;
    explicit_empty["packet_encoding"] = json!("");
    let xudp = parse(&json_source(vec![explicit_xudp]));
    let empty = parse(&json_source(vec![explicit_empty]));
    assert_eq!(
        implicit.nodes[0].content_digest,
        xudp.nodes[0].content_digest
    );
    assert_ne!(
        implicit.nodes[0].content_digest,
        empty.nodes[0].content_digest
    );
    assert_eq!(
        implicit.nodes[0].identity_fingerprint,
        empty.nodes[0].identity_fingerprint
    );
    assert_eq!(config(&empty.nodes[0])["packet_encoding"], "");
}

#[test]
fn necessary_unknown_parameters_are_retained_as_unsupported_without_poisoning_neighbors() {
    let mut unknown = ss("未知参数", PASSWORD);
    unknown["TEST_ONLY-secret-key"] = json!(PASSWORD);
    let mut dependent = ss("原配置依赖", PASSWORD);
    dependent["detour"] = json!("TEST_ONLY-secret-tag");
    let mut file = json!({"type":"trojan","server":"t.example.com","server_port":443,"password":PASSWORD,"tls":{"enabled":true,"certificate_path":"/TEST_ONLY-secret.pem"}});
    file["tag"] = json!("证书文件");
    let batch = parse(&json_source(vec![
        ss("正常", PASSWORD),
        unknown,
        dependent,
        file,
        json!({"type":"naive","server":"n.example.com","server_port":443,"password":PASSWORD}),
        json!({"type":"ssr","server":"r.example.com","server_port":443,"password":PASSWORD}),
    ]));
    assert_eq!(batch.supported_count, 1);
    assert_eq!(batch.unsupported_count, 5);
    let codes = batch
        .nodes
        .iter()
        .skip(1)
        .map(|node| node.preview.unsupported_reasons[0].code.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        codes,
        [
            "unsupported_parameter",
            "unsupported_dependency",
            "unsupported_dependency",
            "unsupported_runtime_capability",
            "unsupported_protocol"
        ]
    );
    assert!(
        batch
            .nodes
            .iter()
            .skip(1)
            .all(|node| node.outbound.is_none()
                && node.content_digest.is_none()
                && node.identity_fingerprint.is_none())
    );
    let public = serde_json::to_string(
        &batch
            .nodes
            .iter()
            .map(|node| &node.preview)
            .collect::<Vec<_>>(),
    )
    .expect("preview");
    assert!(!public.contains("TEST_ONLY"));
    let unknown_query = parse(&format!(
        "trojan://{PASSWORD}@t.example.com:443?TEST_ONLY-key=TEST_ONLY-value"
    ));
    assert_eq!(unknown_query.unsupported_count, 1);
    assert!(
        !serde_json::to_string(&unknown_query.nodes[0].preview)
            .expect("preview")
            .contains("TEST_ONLY")
    );
}

#[test]
fn public_names_and_endpoints_never_echo_urls_or_authentication() {
    let secret_uri = format!("trojan://{PASSWORD}@edge.example.com:443?token=TEST_ONLY-token");
    let mut bad_host = ss("正常名称", PASSWORD);
    bad_host["server"] = json!(secret_uri);
    let mut bad_sni = json!({"type":"trojan","tag":PASSWORD,"server":"edge.example.com","server_port":443,"password":PASSWORD,"tls":{"enabled":true,"server_name":"sni.example.com?token=TEST_ONLY-token"}});
    bad_sni["tag"] = json!(PASSWORD);
    let batch = parse(&json_source(vec![
        ss(&secret_uri, PASSWORD),
        ss(PASSWORD, PASSWORD),
        bad_host,
        bad_sni,
        json!({"type":"vless","tag":UUID,"server":"v.example.com","server_port":443,"uuid":UUID}),
    ]));
    for (index, node) in batch.nodes.iter().enumerate() {
        if index != 2 {
            assert_eq!(node.preview.name, format!("节点 {}", index + 1));
        }
    }
    assert!(batch.nodes[2].preview.server.is_none());
    assert!(batch.nodes[3].preview.sni.is_none());
    let unknown = parse(
        "unknown://TEST_ONLY-secret@edge.example.com:443#https%3A%2F%2Fhidden.example.com%2Faccount%3Ftoken%3DTEST_ONLY-secret",
    );
    assert_eq!(unknown.nodes[0].preview.name, "节点 1");
    let fragment = parse(&format!(
        "trojan://{PASSWORD}@edge.example.com:443#https%3A%2F%2Fhidden.example.com%2Ftoken"
    ));
    assert_eq!(fragment.nodes[0].preview.name, "节点 1");
    let unsupported_auth = parse(&format!(
        "trojan://{PASSWORD}@edge.example.com:443?unknown=1#{PASSWORD}"
    ));
    assert_eq!(unsupported_auth.nodes[0].preview.name, "节点 1");
    for nodes in [
        &batch.nodes,
        &unknown.nodes,
        &fragment.nodes,
        &unsupported_auth.nodes,
    ] {
        let preview =
            serde_json::to_string(&nodes.iter().map(|node| &node.preview).collect::<Vec<_>>())
                .expect("preview");
        for secret in [
            PASSWORD,
            UUID,
            "TEST_ONLY-token",
            "hidden.example.com",
            "://",
        ] {
            assert!(!preview.contains(secret));
        }
    }
    assert_eq!(display_name(Some("中文合法节点"), 0), "中文合法节点");
    for unsafe_name in [
        "/private/token",
        "hk?token=secret",
        "user@host",
        "token=secret",
        "line\nsecret",
    ] {
        assert_eq!(display_name(Some(unsafe_name), 0), "节点 1");
    }
}

#[test]
fn global_configuration_groups_and_provider_urls_are_not_imported_or_executed() {
    let input = json!({"inbounds":[{"type":"tun","interface_name":"TEST_ONLY-interface"}],"dns":{"servers":[{"server":"TEST_ONLY-dns"}]},
        "experimental":{"clash_api":{"secret":PASSWORD}},"outbounds":[{"type":"direct","tag":"DIRECT"},{"type":"selector","outbounds":["edge"]},ss("edge",PASSWORD)]});
    let batch = parse(&input.to_string());
    assert_eq!(batch.nodes.len(), 1);
    assert_eq!(batch.warnings[0].code, "global_configuration_ignored");
    assert_eq!(config(&batch.nodes[0])["server"], "edge.example.com");
    let provider = failure(
        "proxy-providers:\n  remote:\n    type: http\n    url: https://provider.example.com/TEST_ONLY-token\n",
        FormatHint::Auto,
    );
    assert_eq!(provider.code, "provider_only");
    assert!(!provider.to_string().contains("TEST_ONLY"));
    for input in ["{\"outbounds\":[]}", "proxies: []", "payload: []"] {
        let empty = parse(input);
        assert!(empty.nodes.is_empty());
        assert_eq!(empty.supported_count, 0);
        assert_eq!(empty.unsupported_count, 0);
    }
    let double_encoded = failure(
        &STANDARD.encode(STANDARD.encode("ss://broken@edge.example.com:443")),
        FormatHint::Base64UriList,
    );
    assert_eq!(double_encoded.code, "invalid_document");
}

#[test]
fn bounded_json_rejects_duplicates_depth_scalar_body_and_node_count() {
    for input in [
        "{\"outbounds\":[],\"outbounds\":[]}",
        "{\"outbounds\":[{\"type\":\"ss\",\"password\":\"TEST_ONLY-one\",\"password\":\"TEST_ONLY-two\"}]}",
    ] {
        assert_eq!(
            failure(input, FormatHint::SingBoxJson).code,
            "duplicate_field"
        );
    }
    let depth = format!(
        "{{\"unused\":{}0{},\"outbounds\":[]}}",
        "[".repeat(MAX_DEPTH),
        "]".repeat(MAX_DEPTH)
    );
    assert_eq!(
        failure(&depth, FormatHint::SingBoxJson).code,
        "resource_limit"
    );
    let scalar = json!({"unused":"x".repeat(MAX_SCALAR_BYTES+1),"outbounds":[]}).to_string();
    assert_eq!(
        failure(&scalar, FormatHint::SingBoxJson).code,
        "resource_limit"
    );
    assert_eq!(
        failure(&"x".repeat(MAX_BODY_BYTES + 1), FormatHint::Auto).code,
        "resource_limit"
    );
    let many = json_source(vec![ss("节点", PASSWORD); MAX_NODES + 1]);
    assert_eq!(
        failure(&many, FormatHint::SingBoxJson).code,
        "resource_limit"
    );
    let valid = parse(&json_source(vec![ss("节点", PASSWORD); MAX_NODES]));
    assert_eq!(valid.supported_count, MAX_NODES);
}

#[test]
fn yaml_anchors_merges_and_provider_payload_are_bounded_before_expansion() {
    let yaml = format!(
        "defaults: &base\n  type: ss\n  server: edge.example.com\n  port: 443\n  cipher: aes-128-gcm\n  password: {PASSWORD}\nproxies:\n  - <<: *base\n    name: 第一节点\n  - <<: *base\n    name: 第二节点\n    password: TEST_ONLY-rotated\n"
    );
    let batch = parse(&yaml);
    assert_eq!(batch.supported_count, 2);
    assert_eq!(
        batch.nodes[0].identity_fingerprint,
        batch.nodes[1].identity_fingerprint
    );
    assert_ne!(batch.nodes[0].content_digest, batch.nodes[1].content_digest);
    let payload = format!(
        "payload:\n - {{name: 节点, type: ss, server: edge.example.com, port: 443, cipher: aes-128-gcm, password: {PASSWORD}}}\n"
    );
    assert_eq!(parse(&payload).supported_count, 1);
    assert_eq!(parse(&json!({"payload":[{"name":"节点","type":"ss","server":"edge.example.com","port":443,"cipher":"aes-128-gcm","password":PASSWORD}]}).to_string()).format,SubscriptionFormat::ClashYaml);
    assert_eq!(parse(&format!("{{proxies: [{{type: ss, server: edge.example.com, port: 443, cipher: aes-128-gcm, password: {PASSWORD}}}]}}" )).supported_count,1);
    assert_eq!(
        failure("proxies: &self [*self]", FormatHint::ClashYaml).code,
        "invalid_document"
    );
    assert_eq!(
        failure(
            "proxies:\n - type: ss\n   password: TEST_ONLY-one\n   password: TEST_ONLY-two\n",
            FormatHint::ClashYaml
        )
        .code,
        "duplicate_field"
    );
    let mut bomb = "a0: &a0 [\"xxxxxxxxxxxxxxxx\"]\n".to_owned();
    for index in 1..18 {
        bomb.push_str(&format!(
            "a{index}: &a{index} [*a{}, *a{}]\n",
            index - 1,
            index - 1
        ));
    }
    bomb.push_str("proxies: []\n");
    assert_eq!(failure(&bomb, FormatHint::ClashYaml).code, "resource_limit");
    let deep = format!(
        "unused: {}0{}\nproxies: []\n",
        "[".repeat(MAX_DEPTH),
        "]".repeat(MAX_DEPTH)
    );
    assert_eq!(failure(&deep, FormatHint::ClashYaml).code, "resource_limit");
    let large_scalar = format!(
        "unused: '{}'\nproxies: []\n",
        "x".repeat(MAX_SCALAR_BYTES + 1)
    );
    assert_eq!(
        failure(&large_scalar, FormatHint::ClashYaml).code,
        "resource_limit"
    );
    assert_eq!(
        failure("proxies: []\n---\nproxies: []\n", FormatHint::ClashYaml).code,
        "invalid_document"
    );
    assert_eq!(
        failure("proxies: !execute []\n", FormatHint::ClashYaml).code,
        "invalid_document"
    );
}

#[test]
fn clash_protocol_conversion_preserves_transport_and_rejects_non_equivalent_security() {
    let yaml = format!(
        "proxies:\n - name: VMess\n   type: vmess\n   server: vm.example.com\n   port: 443\n   uuid: {UUID}\n   alterId: 2\n   cipher: aes-128-gcm\n   tls: true\n   servername: sni.example.com\n   client-fingerprint: firefox\n   network: ws\n   ws-opts:\n     path: /ws\n     headers: {{Host: sni.example.com, Authorization: {PASSWORD}}}\n     max-early-data: 1024\n     early-data-header-name: Sec-WebSocket-Protocol\n - name: HY2\n   type: hysteria2\n   server: hy.example.com\n   port: 443\n   password: {PASSWORD}\n   sni: hy.example.com\n   obfs: salamander\n   obfs-password: TEST_ONLY-obfs\n   up: 25\n   down: 50\n   ports: 443-445\n - name: TUIC\n   type: tuic\n   server: tuic.example.com\n   port: 443\n   uuid: {UUID}\n   password: {PASSWORD}\n   congestion-controller: bbr\n   udp-relay-mode: quic\n   heartbeat-interval: 12000\n - name: AnyTLS\n   type: anytls\n   server: any.example.com\n   port: 443\n   password: {PASSWORD}\n   idle-session-timeout: 60\n   min-idle-session: 2\n"
    );
    let batch = parse(&yaml);
    assert_eq!(batch.supported_count, 4);
    assert_eq!(
        config(&batch.nodes[0])["transport"]["headers"]["authorization"],
        json!([PASSWORD])
    );
    assert_eq!(config(&batch.nodes[1])["server_ports"], json!(["443:445"]));
    assert_eq!(config(&batch.nodes[2])["heartbeat"], "12s");
    assert_eq!(config(&batch.nodes[3])["idle_session_timeout"], "60s");
    let bad = parse(&format!(
        "proxies:\n - name: 不能忽略证书指纹\n   type: trojan\n   server: t.example.com\n   port: 443\n   password: {PASSWORD}\n   fingerprint: TEST_ONLY-pin\n - name: 不能忽略复用\n   type: vmess\n   server: vm.example.com\n   port: 443\n   uuid: {UUID}\n   smux: {{enabled: true}}\n"
    ));
    assert_eq!(bad.unsupported_count, 2);
    assert!(
        bad.nodes
            .iter()
            .all(|node| node.preview.unsupported_reasons[0].code == "unsupported_parameter")
    );
    let public = serde_json::to_string(
        &bad.nodes
            .iter()
            .map(|node| &node.preview)
            .collect::<Vec<_>>(),
    )
    .expect("preview");
    assert!(!public.contains("TEST_ONLY"));
}

#[test]
fn provider_ids_are_read_from_sing_box_json_and_mihomo_like_numbered_sources() {
    let mut first = ss("first", PASSWORD);
    first["provider_id"] = json!("provider-node-1");
    let batch = parse(&json_source(vec![first.clone()]));
    assert_eq!(batch.supported_count, 1);
    assert_eq!(
        batch.nodes[0].provider_metadata_id.as_deref(),
        Some("provider-node-1")
    );
    // The provider id is metadata, not part of the connection configuration.
    let mut plain = first;
    plain.as_object_mut().unwrap().remove("provider_id");
    assert_eq!(
        batch.nodes[0].content_digest,
        parse(&json_source(vec![plain])).nodes[0].content_digest
    );
    let yaml = format!(
        "proxies:\n  - name: first\n    type: ss\n    server: edge.example.com\n    port: 443\n    cipher: aes-128-gcm\n    password: {PASSWORD}\n    provider_id: provider-node-1\n"
    );
    let clash = parse(&yaml);
    assert_eq!(clash.supported_count, 1);
    assert_eq!(
        clash.nodes[0].provider_metadata_id.as_deref(),
        Some("provider-node-1")
    );
}

#[test]
fn invalid_provider_ids_make_only_that_node_unsupported() {
    for invalid in [
        json!(""),
        json!("   "),
        json!(7),
        json!("a".repeat(257)),
        json!("bad\nid"),
    ] {
        let mut node = ss("broken", PASSWORD);
        node["provider_id"] = invalid.clone();
        let batch = parse(&json_source(vec![node, ss("kept", "TEST_ONLY-other")]));
        assert_eq!(batch.supported_count, 1, "{invalid}");
        let broken = &batch.nodes[0];
        assert_eq!(broken.preview.parse_status, ParseStatus::Unsupported);
        assert!(broken.outbound.is_none() && broken.provider_metadata_id.is_none());
        assert!(
            broken
                .preview
                .unsupported_reasons
                .iter()
                .any(|reason| reason.code == "invalid_provider_identity")
        );
    }
}

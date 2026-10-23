use super::*;
use apsp_compiler::{
    BbrProfile, Hysteria2Masquerade, MultiplexProtocol, SnellMode, TlsVersion, TuicUdpRelayMode,
};

pub(super) fn configured_nodes() -> Vec<Node> {
    let mut model = super::configured_nodes();
    for node in &mut model {
        if node.protocol_config.uses_tcp() {
            node.settings.tcp_keep_alive_seconds = Some(45);
            node.settings.tcp_keep_alive_interval_seconds = Some(15);
        }
        if node.protocol_config.tls().is_some() {
            node.settings.tls_min_version = Some(TlsVersion::V13);
            node.settings.tls_max_version = Some(TlsVersion::V13);
            if node.protocol_config.uses_tcp() {
                node.settings.tls_handshake_timeout_seconds = Some(8);
            }
        }
    }
    model[0].settings.hysteria2.bbr_profile = BbrProfile::Conservative;
    model[0].settings.hysteria2.masquerade = Some(Hysteria2Masquerade {
        status_code: 200,
        content_type: "text/plain; charset=utf-8".into(),
        content: "TEST_ONLY masquerade body".into(),
    });
    model[1].settings.shadowsocks.udp_over_tcp = true;
    let mux = &mut model[1].settings.shadowsocks.multiplex;
    mux.enabled = true;
    mux.padding = true;
    mux.protocol = MultiplexProtocol::Smux;
    mux.max_connections = Some(4);
    mux.min_streams = Some(8);
    model[3].settings.tuic.udp_relay_mode = TuicUdpRelayMode::UdpOverStream;
    model[4].settings.anytls.padding_scheme = vec![
        "stop=4".into(),
        "0=30-30".into(),
        "2=100-400,c,500-800".into(),
    ];
    model[6].settings.snell.mode = SnellMode::Unshaped;
    model[6].settings.snell.reuse = true;
    model
}

#[test]
fn options_preserve_client_server_directions_and_omit_runtime_incompatible_fields() {
    let model = configured_nodes();
    let server: Value = serde_json::from_str(&compile_server(&model).unwrap()).unwrap();
    let client: Value = serde_json::from_str(&compile_client(&model, 1).unwrap()).unwrap();
    let ins = &server["inbounds"];
    let outs = &client["outbounds"];
    assert_eq!(ins[0]["bbr_profile"], "conservative");
    assert_eq!(outs[1]["bbr_profile"], "conservative");
    assert!(ins[0]["masquerade"].get("status_code").is_none());
    assert_eq!(
        ins[0]["masquerade"]["headers"]["Content-Type"],
        "text/plain; charset=utf-8"
    );
    assert!(outs[1].get("masquerade").is_none());
    assert_eq!(ins[1]["multiplex"], json!({"enabled":true,"padding":true}));
    assert_eq!(outs[2]["multiplex"]["protocol"], "smux");
    assert_eq!(outs[2]["multiplex"]["max_connections"], 4);
    assert_eq!(outs[2]["udp_over_tcp"], json!({"enabled":true,"version":2}));
    assert!(ins[1].get("udp_over_tcp").is_none());
    assert_eq!(ins[1]["tcp_keep_alive"], "45s");
    assert_eq!(ins[1]["tcp_keep_alive_interval"], "15s");
    assert!(outs[2].get("tcp_keep_alive").is_none());
    assert_eq!(outs[4]["udp_over_stream"], true);
    assert!(outs[4].get("udp_relay_mode").is_none());
    assert!(ins[3].get("udp_over_stream").is_none());
    assert_eq!(
        ins[4]["padding_scheme"],
        json!(model[4].settings.anytls.padding_scheme)
    );
    assert!(outs[5].get("padding_scheme").is_none());
    assert_eq!(ins[4]["tls"]["handshake_timeout"], "8s");
    assert!(outs[5]["tls"].get("handshake_timeout").is_none());
    assert_eq!(outs[5]["tls"]["min_version"], "1.3");
    assert_eq!(ins[5]["tls"]["min_version"], "1.3");
    for field in ["alpn", "min_version", "max_version", "handshake_timeout"] {
        assert!(outs[6]["tls"].get(field).is_none());
    }
    assert_eq!(ins[6]["mode"], "unshaped");
    assert_eq!(outs[7]["mode"], "unshaped");
    assert_eq!(outs[7]["reuse"], true);
    assert!(ins[6].get("reuse").is_none());
    let mut reversed = model.clone();
    reversed.reverse();
    assert_eq!(
        compile_server(&model).unwrap(),
        compile_server(&reversed).unwrap()
    );
    assert_eq!(
        compile_client(&model, 1).unwrap(),
        compile_client(&reversed, 1).unwrap()
    );
}

#[test]
fn invalid_advanced_options_are_atomic_and_do_not_silently_drop_parameters() {
    for (index, settings) in [
        (0, json!({"disable_tcp_keep_alive":true})),
        (0, json!({"tls_handshake_timeout_seconds":5})),
        (0, json!({"tls_max_version":"1.2"})),
        (4, json!({"tls_min_version":"1.3","tls_max_version":"1.2"})),
        (
            4,
            json!({"disable_tcp_keep_alive":true,"tcp_keep_alive_seconds":10}),
        ),
        (4, json!({"tcp_keep_alive_interval_seconds":0})),
        (4, json!({"tls_handshake_timeout_seconds":3601})),
        (4, json!({"anytls":{"padding_scheme":["0=1-10"]}})),
        (4, json!({"anytls":{"padding_scheme":["stop=2","2=1-10"]}})),
        (
            4,
            json!({"anytls":{"padding_scheme":["stop=2","0=1-10","0=2-20"]}}),
        ),
        (4, json!({"anytls":{"padding_scheme":["stop=2","0=10-1"]}})),
        (
            4,
            json!({"anytls":{"padding_scheme":["stop=2","0=1-999999"]}}),
        ),
        (4, json!({"anytls":{"padding_scheme":["stop=2","0=0-10"]}})),
        (
            0,
            json!({"hysteria2":{"masquerade":{"status_code":199,"content_type":"text/plain","content":"x"}}}),
        ),
        (
            0,
            json!({"hysteria2":{"masquerade":{"status_code":404,"content_type":"text/plain","content":"x"}}}),
        ),
        (
            0,
            json!({"hysteria2":{"masquerade":{"status_code":200,"content_type":"text/plain\r\nInjected: 1","content":"x"}}}),
        ),
        (1, json!({"shadowsocks":{"multiplex":{"padding":true}}})),
        (
            1,
            json!({"shadowsocks":{"multiplex":{"enabled":true,"max_connections":1,"max_streams":1}}}),
        ),
        (
            1,
            json!({"shadowsocks":{"multiplex":{"enabled":true,"min_streams":0}}}),
        ),
        (1, json!({"snell":{"reuse":true}})),
        (6, json!({"shadowsocks":{"udp_over_tcp":true}})),
        (6, json!({"tls_min_version":"1.2"})),
    ] {
        let mut model = nodes();
        model[index].settings = serde_json::from_value(settings.clone()).unwrap();
        assert!(
            compile_server(&model).is_err(),
            "server accepted {settings}"
        );
        assert!(
            compile_client(&model, 1).is_err(),
            "client accepted {settings}"
        );
    }
}

#[test]
fn empty_optional_controls_restore_native_defaults_and_alternate_modes_render() {
    let mut model = nodes();
    model[4].settings.anytls.padding_scheme = vec!["stop=8".into()];
    model[3].settings.tuic.udp_relay_mode = TuicUdpRelayMode::QuicStream;
    model[6].settings.snell.mode = SnellMode::UnsafeRaw;
    model[6].settings.disable_tcp_keep_alive = true;
    model[0].settings.hysteria2.masquerade = Some(Hysteria2Masquerade {
        status_code: 404,
        content_type: String::new(),
        content: "TEST_ONLY missing".into(),
    });
    let native: Value = serde_json::from_str(&compile_server(&model).unwrap()).unwrap();
    assert_eq!(native["inbounds"][0]["masquerade"]["status_code"], 404);
    assert!(native["inbounds"][0]["masquerade"].get("headers").is_none());
    assert_eq!(native["inbounds"][6]["disable_tcp_keep_alive"], true);
    let client: Value = serde_json::from_str(&compile_client(&model, 1).unwrap()).unwrap();
    assert_eq!(client["outbounds"][4]["udp_relay_mode"], "quic");
    assert!(client["outbounds"][4].get("udp_over_stream").is_none());
    for node in &mut model {
        node.settings = Default::default();
    }
    assert_eq!(
        compile_server(&model).unwrap(),
        compile_server(&nodes()).unwrap()
    );
}

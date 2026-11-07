use super::*;
use crate::{Node, ProtocolConfig};
use serde_json::{Value, json};

fn seconds(target: &mut Value, field: &str, value: Option<u16>) {
    if let Some(value) = value {
        target[field] = json!(format!("{value}s"));
    }
}

pub(crate) fn apply(node: &Node, config: &mut Value, client: bool) {
    let settings = &node.settings;
    if !client {
        config["listen"] = json!(settings.listen);
        if settings.tcp_fast_open {
            config["tcp_fast_open"] = json!(true);
        }
        if settings.disable_tcp_keep_alive {
            config["disable_tcp_keep_alive"] = json!(true);
        }
        seconds(config, "tcp_keep_alive", settings.tcp_keep_alive_seconds);
        seconds(
            config,
            "tcp_keep_alive_interval",
            settings.tcp_keep_alive_interval_seconds,
        );
    }
    if let Some(transport) = settings.transport.native(client) {
        config["transport"] = transport;
    }
    // Naive negotiates HTTP/2 itself and rejects an explicit client ALPN.
    if !settings.tls_alpn.is_empty()
        && !(client && matches!(node.protocol_config, ProtocolConfig::Naive { .. }))
    {
        config["tls"]["alpn"] = json!(settings.tls_alpn);
    }
    // Cronet owns the Naive client's TLS policy; these settings remain server-side.
    if !(client && matches!(node.protocol_config, ProtocolConfig::Naive { .. })) {
        if let Some(version) = settings.tls_min_version {
            config["tls"]["min_version"] = json!(version);
        }
        if let Some(version) = settings.tls_max_version {
            config["tls"]["max_version"] = json!(version);
        }
        if !client && settings.tls_handshake_timeout_seconds.is_some() {
            seconds(
                &mut config["tls"],
                "handshake_timeout",
                settings.tls_handshake_timeout_seconds,
            );
        }
    }
    match &node.protocol_config {
        ProtocolConfig::VlessReality => {
            if client {
                config["tls"]["utls"]["fingerprint"] = json!(settings.reality.fingerprint);
            } else {
                config["tls"]["reality"]["handshake"] = json!({
                    "server": crate::unbracket_host(settings.reality.handshake_server.as_deref().unwrap_or(&node.sni)),
                    "server_port": settings.reality.handshake_port
                });
                seconds(
                    &mut config["tls"]["reality"],
                    "max_time_difference",
                    settings.reality.max_time_difference_seconds,
                );
            }
        }
        ProtocolConfig::Hysteria2 { .. } => {
            let hy = &settings.hysteria2;
            if let Some(realm) = &settings.realm {
                config["realm"] = json!({"server_url":realm.server_url,"realm_id":realm.realm_id,"token":if client{&realm.client_token}else{&realm.server_token},"stun_servers":realm.stun_servers,"ip_version":realm.ip_version});
            }
            if let (Some(up), Some(down)) = (hy.up_mbps, hy.down_mbps) {
                config["up_mbps"] = json!(if client { down } else { up });
                config["down_mbps"] = json!(if client { up } else { down });
            }
            if !client && hy.ignore_client_bandwidth {
                config["ignore_client_bandwidth"] = json!(true);
            }
            if let Some(password) = &hy.obfs_password {
                config["obfs"] = json!({"type": "salamander", "password": password});
            }
            if hy.bbr_profile != BbrProfile::default() {
                config["bbr_profile"] = json!(hy.bbr_profile);
            }
            if !client && let Some(masquerade) = &hy.masquerade {
                config["masquerade"] = json!({"type":"string", "content":masquerade.content});
                // Upstream writes an explicit status before headers. Implicit 200 preserves
                // Content-Type; other statuses must leave the content type to the runtime.
                if masquerade.status_code == 200 {
                    config["masquerade"]["headers"] =
                        json!({"Content-Type":masquerade.content_type});
                } else {
                    config["masquerade"]["status_code"] = json!(masquerade.status_code);
                }
            }
        }
        ProtocolConfig::Tuic { .. } => {
            let tuic = &settings.tuic;
            if tuic.congestion_control != CongestionControl::default() {
                config["congestion_control"] = json!(tuic.congestion_control);
            }
            if tuic.zero_rtt_handshake {
                config["zero_rtt_handshake"] = json!(true);
            }
            seconds(config, "heartbeat", tuic.heartbeat_seconds);
            if !client {
                seconds(config, "auth_timeout", tuic.auth_timeout_seconds);
            } else {
                match tuic.udp_relay_mode {
                    TuicUdpRelayMode::Native => {}
                    TuicUdpRelayMode::QuicStream => config["udp_relay_mode"] = json!("quic"),
                    TuicUdpRelayMode::UdpOverStream => config["udp_over_stream"] = json!(true),
                }
            }
        }
        ProtocolConfig::Anytls { .. } => {
            let value = &settings.anytls;
            if !client {
                if !value.padding_scheme.is_empty() {
                    config["padding_scheme"] = json!(value.padding_scheme);
                }
                return;
            }
            seconds(
                config,
                "idle_session_check_interval",
                value.idle_session_check_seconds,
            );
            seconds(
                config,
                "idle_session_timeout",
                value.idle_session_timeout_seconds,
            );
            if let Some(count) = value.min_idle_session {
                config["min_idle_session"] = json!(count);
            }
        }
        ProtocolConfig::SnellV6 { .. } => {
            if settings.snell.mode != SnellMode::default() {
                config["mode"] = json!(settings.snell.mode);
            }
            if client && settings.snell.reuse {
                config["reuse"] = json!(true);
            }
        }
        ProtocolConfig::Shadowsocks2022 { .. } => {
            let ss = &settings.shadowsocks;
            if client && ss.udp_over_tcp {
                config["udp_over_tcp"] = json!({"enabled":true, "version":2});
            }
            let mux = &ss.multiplex;
            if mux.enabled {
                let mut native = json!({"enabled":true, "padding":mux.padding});
                if client {
                    native["protocol"] = json!(mux.protocol);
                    for (field, value) in [
                        ("max_connections", mux.max_connections),
                        ("min_streams", mux.min_streams),
                        ("max_streams", mux.max_streams),
                    ] {
                        if let Some(value) = value {
                            native[field] = json!(value);
                        }
                    }
                }
                config["multiplex"] = native;
            }
        }
        _ => {}
    }
}

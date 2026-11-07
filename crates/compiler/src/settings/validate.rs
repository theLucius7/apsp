use super::*;
use crate::{CompileError, Node, ProtocolConfig, invalid_node, valid_public_host};
use std::{collections::BTreeSet, net::IpAddr};

pub(crate) fn validate(node: &Node) -> Result<(), CompileError> {
    let value = &node.settings;
    let fail = |reason| invalid_node(node, reason);
    let address: IpAddr = value
        .listen
        .parse()
        .map_err(|_| fail("监听地址必须是 IPv4 或 IPv6 地址"))?;
    if address.is_multicast() || address == IpAddr::from([255, 255, 255, 255]) {
        return Err(fail("监听地址不能是组播或广播地址"));
    }
    if value.public_port == Some(0) {
        return Err(fail("公开端口必须为 1 至 65535"));
    }
    let tcp_configured = value.tcp_fast_open
        || value.disable_tcp_keep_alive
        || value.tcp_keep_alive_seconds.is_some()
        || value.tcp_keep_alive_interval_seconds.is_some();
    if tcp_configured && !node.protocol_config.uses_tcp() {
        return Err(fail("UDP 入站不支持 TCP Fast Open 和 TCP 保活参数"));
    }
    if value.disable_tcp_keep_alive
        && (value.tcp_keep_alive_seconds.is_some()
            || value.tcp_keep_alive_interval_seconds.is_some())
    {
        return Err(fail("关闭 TCP 保活时不能同时设置保活时间或探测间隔"));
    }
    let tls_configured = !value.tls_alpn.is_empty()
        || value.tls_min_version.is_some()
        || value.tls_max_version.is_some()
        || value.tls_handshake_timeout_seconds.is_some();
    if tls_configured && node.protocol_config.tls().is_none() && !node.protocol_config.is_reality()
    {
        return Err(fail("当前协议不支持 TLS 参数"));
    }
    if node.protocol_config.is_reality()
        && (!value.tls_alpn.is_empty()
            || value.tls_min_version.is_some()
            || value.tls_max_version.is_some())
    {
        return Err(fail(
            "Reality 使用传输默认 TLS 协商，仅证书协议支持自定义 ALPN 和 TLS 版本",
        ));
    }
    if value
        .tls_min_version
        .zip(value.tls_max_version)
        .is_some_and(|(min, max)| min > max)
    {
        return Err(fail("TLS 最低版本不能高于最高版本"));
    }
    if (node.protocol_config.is_reality() || !node.protocol_config.uses_tcp())
        && value.tls_max_version == Some(TlsVersion::V12)
    {
        return Err(fail("Reality 和 QUIC 协议必须允许 TLS 1.3"));
    }
    if !node.protocol_config.uses_tcp() && value.tls_handshake_timeout_seconds.is_some() {
        return Err(fail("TLS 握手超时只适用于 TCP TLS 协议"));
    }
    value.transport.validate(node)?;
    let mut alpn = BTreeSet::new();
    if value.tls_alpn.len() > 8
        || value.tls_alpn.iter().any(|entry| {
            entry.is_empty()
                || entry.len() > 32
                || !entry.bytes().all(|b| b.is_ascii_graphic())
                || !alpn.insert(entry)
        })
    {
        return Err(fail(
            "ALPN 需要 1 至 8 个不重复的可见 ASCII 标识，每个最多 32 字节",
        ));
    }
    if matches!(node.protocol_config, ProtocolConfig::Naive { .. })
        && !value.tls_alpn.is_empty()
        && value.tls_alpn != ["h2"]
    {
        return Err(fail("Naive HTTP/2 的 ALPN 仅支持 h2"));
    }
    let reality = &value.reality;
    if !node.protocol_config.is_reality() && *reality != RealitySettings::default() {
        return Err(fail("当前协议不支持 Reality 参数"));
    }
    if reality.handshake_port == 0
        || reality
            .handshake_server
            .as_ref()
            .is_some_and(|host| !valid_public_host(host))
    {
        return Err(fail(
            "Reality 握手目标需为不含端口的域名或 IP，端口需为 1 至 65535",
        ));
    }
    if let Some(realm) = &value.realm
        && (!matches!(
            node.protocol_config,
            crate::ProtocolConfig::Hysteria2 { .. }
        ) || !realm.server_url.starts_with("https://")
            || realm.server_url.len() > 2048
            || realm.server_url.contains(['@', '#', '\n', '\r'])
            || realm.realm_id.is_empty()
            || realm.realm_id.len() > 128
            || realm.realm_id.chars().any(char::is_control)
            || ![0, 4, 6].contains(&realm.ip_version)
            || realm.stun_servers.is_empty()
            || realm.stun_servers.len() > 8
            || realm
                .stun_servers
                .iter()
                .any(|s| s.len() > 255 || s.chars().any(char::is_whitespace))
            || [&realm.server_token, &realm.client_token]
                .iter()
                .any(|s| s.len() < 32 || s.len() > 4096 || s.chars().any(char::is_control)))
    {
        return Err(crate::invalid_node(
            node,
            "invalid Hysteria Realm parameters",
        ));
    }
    let hy = &value.hysteria2;
    if !matches!(node.protocol_config, ProtocolConfig::Hysteria2 { .. })
        && *hy != Hysteria2Settings::default()
    {
        return Err(fail("当前协议不支持 Hysteria2 参数"));
    }
    if hy.up_mbps.is_some() != hy.down_mbps.is_some()
        || [hy.up_mbps, hy.down_mbps]
            .into_iter()
            .flatten()
            .any(|n| !(1..=1_000_000).contains(&n))
        || (hy.ignore_client_bandwidth && hy.up_mbps.is_some())
    {
        return Err(fail(
            "Hysteria2 上下行带宽需同时填写 1 至 1000000 Mbps，且不能同时强制 BBR",
        ));
    }
    if hy.obfs_password.as_ref().is_some_and(|secret| {
        !(8..=256).contains(&secret.len()) || secret.chars().any(char::is_control)
    }) {
        return Err(fail("混淆密码需为 8 至 256 字节且不含控制字符"));
    }
    if !matches!(node.protocol_config, ProtocolConfig::Tuic { .. })
        && value.tuic != TuicSettings::default()
    {
        return Err(fail("当前协议不支持 TUIC 参数"));
    }
    if !matches!(node.protocol_config, ProtocolConfig::Anytls { .. })
        && value.anytls != AnyTlsSettings::default()
    {
        return Err(fail("当前协议不支持 AnyTLS 参数"));
    }
    if !matches!(node.protocol_config, ProtocolConfig::SnellV6 { .. })
        && value.snell != SnellSettings::default()
    {
        return Err(fail("当前协议不支持 Snell 参数"));
    }
    if !matches!(node.protocol_config, ProtocolConfig::Shadowsocks2022 { .. })
        && value.shadowsocks != ShadowsocksSettings::default()
    {
        return Err(fail("当前协议不支持 Shadowsocks 参数"));
    }
    if let Some(masquerade) = &hy.masquerade
        && (!(200..=599).contains(&masquerade.status_code)
            || masquerade.content.len() > 16384
            || masquerade.content.contains('\0')
            || (matches!(masquerade.status_code, 204 | 304) && !masquerade.content.is_empty())
            || (masquerade.status_code == 200 && masquerade.content_type.is_empty())
            || (masquerade.status_code != 200 && !masquerade.content_type.is_empty())
            || masquerade.content_type.len() > 128
            || !masquerade
                .content_type
                .bytes()
                .all(|b| b.is_ascii_graphic() || b == b' '))
    {
        return Err(fail(
            "伪装响应状态为 200 至 599，正文最多 16 KiB；200 需指定 1 至 128 字节内容类型，其他状态须留空以兼容运行时",
        ));
    }
    validate_padding(&value.anytls.padding_scheme).map_err(fail)?;
    let mux = &value.shadowsocks.multiplex;
    if !mux.enabled && *mux != MultiplexSettings::default() {
        return Err(fail("关闭多路复用时不能设置附属复用参数"));
    }
    if [mux.max_connections, mux.min_streams, mux.max_streams]
        .into_iter()
        .flatten()
        .any(|n| !(1..=1024).contains(&n))
        || (mux.max_streams.is_some()
            && (mux.max_connections.is_some() || mux.min_streams.is_some()))
    {
        return Err(fail(
            "复用数量需为 1 至 1024，最大流数不能与最大连接数或最小流数同时设置",
        ));
    }
    if [
        value.tcp_keep_alive_seconds,
        value.tcp_keep_alive_interval_seconds,
        value.tls_handshake_timeout_seconds,
        value.reality.max_time_difference_seconds,
        value.tuic.auth_timeout_seconds,
        value.tuic.heartbeat_seconds,
        value.anytls.idle_session_check_seconds,
        value.anytls.idle_session_timeout_seconds,
    ]
    .into_iter()
    .flatten()
    .any(|n| !(1..=3600).contains(&n))
        || value.anytls.min_idle_session.is_some_and(|n| n > 128)
    {
        return Err(fail(
            "超时和间隔需为 1 至 3600 秒，闲置会话数量需为 0 至 128",
        ));
    }
    Ok(())
}

fn validate_padding(lines: &[String]) -> Result<(), &'static str> {
    const INVALID: &str =
        "填充方案需包含唯一 stop=1..64，包编号小于 stop，尺寸范围为 1 至 65535 或 c；最多 8 KiB";
    if lines.is_empty() {
        return Ok(());
    }
    if lines.len() > 65 || lines.iter().map(|line| line.len() + 1).sum::<usize>() > 8192 {
        return Err(INVALID);
    }
    let mut entries = std::collections::BTreeMap::new();
    for line in lines {
        let (key, value) = line.split_once('=').ok_or(INVALID)?;
        if entries.insert(key, value).is_some() || line.chars().any(char::is_whitespace) {
            return Err(INVALID);
        }
    }
    let stop = entries
        .get("stop")
        .ok_or(INVALID)?
        .parse::<u16>()
        .map_err(|_| INVALID)?;
    if !(1..=64).contains(&stop) {
        return Err(INVALID);
    }
    for (key, value) in entries {
        if key == "stop" {
            continue;
        }
        let index = key.parse::<u16>().map_err(|_| INVALID)?;
        if index >= stop || index.to_string() != key {
            return Err(INVALID);
        }
        for part in value.split(',') {
            if part == "c" {
                continue;
            }
            let (min, max) = part.split_once('-').ok_or(INVALID)?;
            let min = min.parse::<u16>().map_err(|_| INVALID)?;
            let max = max.parse::<u16>().map_err(|_| INVALID)?;
            if min == 0 || min > max {
                return Err(INVALID);
            }
        }
    }
    Ok(())
}

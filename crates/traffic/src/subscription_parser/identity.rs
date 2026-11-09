use super::{
    hex_digest,
    outbound::{NormalizedOutbound, Transport},
};
use serde_json::{Value, json};

pub(super) fn digests(outbound: &NormalizedOutbound) -> (String, String) {
    let content =
        serde_json::to_vec(outbound).expect("normalized outbound serialization is infallible");
    let common = outbound.common();
    let transport = common
        .transport
        .as_ref()
        .map(|transport| match transport {
            Transport::Ws { path, headers, .. } => {
                json!({"type":"ws", "path":path, "host":headers.get("host")})
            }
            Transport::Http {
                host, path, method, ..
            } => json!({"type":"http", "host":host, "path":path, "method":method}),
            Transport::Grpc { service_name, .. } => {
                json!({"type":"grpc", "service_name":service_name})
            }
            Transport::Httpupgrade { host, path, .. } => {
                json!({"type":"httpupgrade", "host":host, "path":path})
            }
            Transport::Quic {} => json!({"type":"quic"}),
        })
        .unwrap_or_else(|| json!({"type":"tcp"}));
    let protocol_identity: Value = match outbound {
        NormalizedOutbound::Shadowsocks {
            method,
            plugin,
            plugin_opts,
            ..
        } => {
            // Only transport identity fields participate; raw plugin options remain private version data.
            let fields = super::outbound::plugin_fields(plugin_opts.as_deref().unwrap_or(""))
                .expect("normalized plugin options were validated");
            let identity = fields
                .into_iter()
                .filter(|(key, _)| {
                    ["mode", "obfs", "host", "obfs-host", "path", "tls"].contains(&key.as_str())
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            json!({"method":method,"plugin":plugin,"transport":identity})
        }
        NormalizedOutbound::Socks { version, .. } => json!({"version":version}),
        NormalizedOutbound::Hysteria2 { server_ports, .. } => json!({"server_ports":server_ports}),
        _ => Value::Null,
    };
    let identity = json!({
        "identity_schema":"apsp-external-endpoint-v1",
        "protocol":outbound.protocol(),
        "server":common.server,
        "server_port":common.server_port,
        "sni":common.tls.as_ref().and_then(|tls| tls.server_name.as_deref()),
        "tls":common.tls.as_ref().is_some_and(|tls| tls.enabled),
        "reality_public_key":common.tls.as_ref().and_then(|tls| tls.reality.as_ref()).filter(|reality| reality.enabled).map(|reality| reality.public_key.as_str()),
        "transport":transport,
        "protocol_identity":protocol_identity,
    });
    let identity = serde_json::to_vec(&identity)
        .expect("public endpoint identity serialization is infallible");
    (hex_digest(&content), hex_digest(&identity))
}

//! Preserve an existing native listener while replacing its accounting identity.
//!
//! Each input is a complete single-listener configuration. Routes and outbound
//! references are namespaced before merging into the one managed runtime.
use crate::{Access, CompileError, Node, ProtocolConfig, stat_name};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreservedNode {
    pub node: Node,
    pub configuration: Value,
}

/// Legacy passwords are wire identities, not newly generated access secrets.
pub fn nexus_access(user_id: i64, uuid: uuid::Uuid, protocol: &ProtocolConfig) -> Access {
    let text = uuid.to_string();
    let credential = match protocol {
        ProtocolConfig::Shadowsocks2022 { method, .. } => {
            STANDARD.encode(&text.as_bytes()[..method.key_size()])
        }
        ProtocolConfig::Hysteria2 { .. } => text,
        _ => String::new(),
    };
    Access {
        user_id,
        uuid,
        credential,
    }
}

fn fail(node: &Node, reason: &'static str) -> CompileError {
    crate::invalid_node(node, reason)
}

fn no_external_files(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.iter().all(|(key, value)| {
            !((key.ends_with("_path") || key == "path")
                && value.as_str().is_some_and(|s| !s.is_empty()))
                && no_external_files(value)
        }),
        Value::Array(values) => values.iter().all(no_external_files),
        _ => true,
    }
}

fn rewrite(value: &mut Value, tags: &BTreeMap<String, String>) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(
                    key.as_str(),
                    "tag" | "outbound" | "detour" | "final" | "default" | "outbounds"
                ) {
                    match value {
                        Value::String(s) => {
                            if let Some(new) = tags.get(s) {
                                *s = new.clone();
                            }
                        }
                        Value::Array(values) => {
                            for v in values {
                                if let Some(new) = v.as_str().and_then(|s| tags.get(s)) {
                                    *v = json!(new);
                                }
                            }
                        }
                        _ => {}
                    }
                } else {
                    rewrite(value, tags);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(|v| rewrite(v, tags)),
        _ => {}
    }
}

/// Route options that describe the host rather than one listener. Their values
/// may reference DNS server tags, which are kept unchanged with the shared DNS.
const HOST_ROUTE_OPTIONS: [&str; 3] = [
    "default_domain_resolver",
    "default_interface",
    "auto_detect_interface",
];

/// Loopback control interface the Agent uses to close sessions beyond a
/// concurrent client address limit; never published beyond the device.
pub const SESSION_CONTROL_LISTEN: &str = "127.0.0.1:18086";

/// Runtime facilities for concurrent client address limits: per-session log
/// lines (read only from the volatile runtime log) and the local control API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionControl {
    pub secret: String,
}

/// Compilation never rotates credentials or falls back to a direct outbound.
pub fn compile_preserved_server(inputs: &[PreservedNode]) -> Result<String, CompileError> {
    compile_preserved_server_with(inputs, None)
}

/// As `compile_preserved_server`, optionally enabling session control.
pub fn compile_preserved_server_with(
    inputs: &[PreservedNode],
    control: Option<&SessionControl>,
) -> Result<String, CompileError> {
    if let Some(control) = control
        && (!(32..=128).contains(&control.secret.len())
            || !control.secret.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(CompileError::InvalidSessionControl);
    }
    let models: Vec<_> = inputs.iter().map(|input| input.node.clone()).collect();
    crate::sorted_validated(&models, true)?;
    let mut inputs: Vec<_> = inputs.iter().collect();
    inputs.sort_by_key(|input| input.node.id);
    let mut inbounds = Vec::new();
    let mut outbounds = Vec::new();
    let mut rules = Vec::new();
    let mut stats = BTreeSet::new();
    let mut common_dns: Option<Value> = None;
    // Host-wide route options (outbound interface and default resolver) apply
    // to every listener, so all merged nodes must carry the same values.
    let mut common_route: Option<serde_json::Map<String, Value>> = None;
    for input in inputs {
        let node = &input.node;
        let config = input
            .configuration
            .as_object()
            .ok_or_else(|| fail(node, "configuration must be an object"))?;
        if config.keys().any(|key| {
            !matches!(
                key.as_str(),
                "log" | "inbounds" | "outbounds" | "route" | "dns"
            )
        }) || !no_external_files(&input.configuration)
        {
            return Err(fail(
                node,
                "configuration must be self-contained; unsupported global settings",
            ));
        }
        if let Some(dns) = config.get("dns").filter(|value| !value.is_null()) {
            if common_dns.as_ref().is_some_and(|previous| previous != dns) {
                return Err(fail(
                    node,
                    "different DNS policies require explicit reconciliation before merging",
                ));
            }
            common_dns = Some(dns.clone());
        }
        let listeners = config
            .get("inbounds")
            .and_then(Value::as_array)
            .ok_or_else(|| fail(node, "missing inbounds"))?;
        if listeners.len() != 1 {
            return Err(fail(node, "each preserved node must have one listener"));
        }
        let mut inbound = listeners[0].clone();
        let expected: Value = serde_json::from_str(&crate::compile_server(&[Node {
            users: vec![nexus_access(
                1,
                uuid::Uuid::from_u128(1),
                &node.protocol_config,
            )],
            enabled: true,
            ..node.clone()
        }])?)?;
        let expected = &expected["inbounds"][0];
        for key in ["type", "listen", "listen_port", "method", "password"] {
            if inbound[key] != expected[key] {
                return Err(fail(
                    node,
                    "preserved listener identity differs from the node model",
                ));
            }
        }
        if node.protocol_config.is_reality() {
            for key in ["private_key", "short_id"] {
                if inbound["tls"]["reality"][key] != expected["tls"]["reality"][key] {
                    return Err(fail(
                        node,
                        "preserved Reality identity differs from the node model",
                    ));
                }
            }
            // Users are regenerated from the model, so a flow mismatch would
            // silently break every issued client link.
            let flow = node.settings.reality.flow.native();
            if inbound["users"].as_array().is_some_and(|users| {
                users
                    .iter()
                    .any(|user| user["flow"].as_str().unwrap_or_default() != flow)
            }) {
                return Err(fail(
                    node,
                    "preserved VLESS flow differs from the node model",
                ));
            }
        }
        let route = config
            .get("route")
            .and_then(Value::as_object)
            .ok_or_else(|| fail(node, "missing route"))?;
        if route.keys().any(|key| {
            !matches!(key.as_str(), "rules" | "final")
                && !HOST_ROUTE_OPTIONS.contains(&key.as_str())
        }) {
            return Err(fail(
                node,
                "global route settings need explicit reconciliation",
            ));
        }
        let options: serde_json::Map<_, _> = route
            .iter()
            .filter(|(key, _)| HOST_ROUTE_OPTIONS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if common_route
            .as_ref()
            .is_some_and(|previous| previous != &options)
        {
            return Err(fail(
                node,
                "different host route options require explicit reconciliation before merging",
            ));
        }
        common_route = Some(options);
        let original = config
            .get("outbounds")
            .and_then(Value::as_array)
            .ok_or_else(|| fail(node, "missing outbounds"))?;
        let mut tags = BTreeMap::new();
        let mut blocked = BTreeSet::new();
        for outbound in original {
            let tag = outbound["tag"]
                .as_str()
                .filter(|tag| !tag.is_empty())
                .ok_or_else(|| fail(node, "outbound tag is required"))?;
            if tags
                .insert(tag.to_owned(), format!("n{}-{tag}", node.id))
                .is_some()
            {
                return Err(fail(node, "duplicate outbound tag"));
            }
            if outbound["type"] == "block" {
                blocked.insert(tag.to_owned());
            }
        }
        let final_tag = route
            .get("final")
            .and_then(Value::as_str)
            .ok_or_else(|| fail(node, "explicit final outbound is required"))?;
        if !tags.contains_key(final_tag) {
            return Err(fail(node, "missing final outbound"));
        }
        if !node.enabled {
            continue;
        }
        let inbound_tag = format!("node-{}", node.id);
        inbound["tag"] = json!(inbound_tag);
        let mut users = node.users.clone();
        users.sort_by_key(|user| user.user_id);
        inbound["users"] = json!(
            users
                .iter()
                .map(|user| {
                    let name = stat_name(user.user_id, node.id);
                    stats.insert(name.clone());
                    match node.protocol_config {
                        ProtocolConfig::VlessReality => {
                            crate::protocols::reality_identity(node, name, user.uuid)
                        }
                        _ => json!({"name":name,"password":user.credential}),
                    }
                })
                .collect::<Vec<_>>()
        );
        // An empty SS multi-user list can be interpreted as a single-user
        // listener. Remove every unauthorized listener instead of exposing it.
        if !users.is_empty() {
            inbounds.push(inbound);
        }
        for outbound in original {
            if outbound["type"] == "block" {
                continue;
            }
            let mut outbound = outbound.clone();
            rewrite(&mut outbound, &tags);
            outbounds.push(outbound);
        }
        let source_rules = match route.get("rules") {
            Some(Value::Array(rules)) => rules.clone(),
            None | Some(Value::Null) => Vec::new(),
            _ => return Err(fail(node, "route rules must be an array")),
        };
        for mut rule in source_rules {
            let object = rule
                .as_object_mut()
                .ok_or_else(|| fail(node, "invalid route rule"))?;
            if object.contains_key("inbound") {
                return Err(fail(
                    node,
                    "existing inbound-scoped rules need explicit reconciliation",
                ));
            }
            let reject = object
                .get("outbound")
                .and_then(Value::as_str)
                .is_some_and(|tag| blocked.contains(tag));
            if reject {
                object.remove("outbound");
                object.insert("action".into(), json!("reject"));
            }
            let mut action = serde_json::Map::new();
            for key in [
                "action",
                "outbound",
                "sniffer",
                "timeout",
                "strategy",
                "server",
                "method",
                "no_drop",
                "override_address",
                "override_port",
            ] {
                if let Some(value) = object.remove(key) {
                    action.insert(key.into(), value);
                }
            }
            if action.is_empty() {
                return Err(fail(node, "route action is required"));
            }
            let mut scoped =
                json!({"type":"logical","mode":"and","rules":[{"inbound":[inbound_tag]},rule]});
            scoped.as_object_mut().unwrap().extend(action);
            rewrite(&mut scoped, &tags);
            rules.push(scoped);
        }
        rules.push(if blocked.contains(final_tag) {
            json!({"inbound":[inbound_tag],"action":"reject"})
        } else {
            json!({"inbound":[inbound_tag],"action":"route","outbound":tags[final_tag]})
        });
    }
    let mut route = common_route.unwrap_or_default();
    route.insert("rules".into(), json!(rules));
    let mut result = json!({"log":{"level":"warn","timestamp":true},"inbounds":inbounds,"outbounds":outbounds,"route":route,
        "experimental":{"v2ray_api":{"listen":"127.0.0.1:18085","stats":{"enabled":true,"users":stats}}}});
    if let Some(dns) = common_dns {
        result["dns"] = dns;
    }
    if let Some(control) = control {
        // Session lines carry client and destination addresses; they reach only
        // the volatile runtime log namespace the Agent reads, never disk.
        result["log"] = json!({"level":"info","timestamp":true});
        result["experimental"]["clash_api"] =
            json!({"external_controller":SESSION_CONTROL_LISTEN,"secret":control.secret});
    }
    crate::pretty(result)
}

use super::model::acceptance_name;
use super::{ManagedAcceptance, OrderedPath, PathHop, ProbeControl};
use crate::{Access, CompileError, Node};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// A server without ordered paths keeps the plain node configuration byte for byte.
pub fn compile_server_with_paths(
    nodes: &[Node],
    paths: &[OrderedPath],
    internal_accepts: &[ManagedAcceptance],
    probe_control: Option<&ProbeControl>,
) -> Result<String, CompileError> {
    if paths.is_empty() && internal_accepts.is_empty() && probe_control.is_none() {
        return crate::compile_server(nodes);
    }
    super::context::validate(nodes, paths, internal_accepts, probe_control)?;
    let original = crate::compile_server(nodes).map_err(|_| CompileError::InvalidPath {
        chain_id: paths
            .first()
            .map(|path| path.chain_id)
            .or_else(|| internal_accepts.first().map(|accept| accept.chain_id))
            .unwrap_or(0),
        position: None,
        reason: "local node configuration is invalid",
    })?;
    overlay(paths, internal_accepts, probe_control, &original)
}

fn overlay(
    paths: &[OrderedPath],
    internal_accepts: &[ManagedAcceptance],
    probe_control: Option<&ProbeControl>,
    original: &str,
) -> Result<String, CompileError> {
    let mut config: Value = serde_json::from_str(original)?;
    let mut paths: Vec<_> = paths.iter().collect();
    paths.sort_by_key(|path| (path.chain_id, path.generation));
    let mut accepts: Vec<_> = internal_accepts.iter().collect();
    accepts.sort_by_key(|accept| (accept.chain_id, accept.generation, accept.position));
    let mut rules = Vec::new();
    for accept in accepts {
        add_acceptance(&mut config, accept)?;
        // Transit identities have no public-node route and never enter stats.
        rules.push(json!({"inbound":[format!("node-{}",accept.endpoint.node.id)],"auth_user":[acceptance_name(accept)],"action":"route","outbound":"direct"}));
    }
    let active_entries: BTreeSet<_> = paths
        .iter()
        .filter(|path| path.active)
        .map(|path| path.entry_node_id)
        .collect();
    for path in &paths {
        let tag = format!("node-{}", path.entry_node_id);
        if config["route"]["rules"].as_array().is_some_and(|rules| {
            rules.iter().any(|rule| {
                rule["inbound"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(&tag)))
            })
        }) {
            return Err(super::validate::error(
                path,
                None,
                "public entry is selected by incompatible path pipelines",
            ));
        }
    }
    let mut blocked_entries = BTreeSet::new();
    for path in &paths {
        for (index, hop) in path.hops.iter().enumerate() {
            let mut outbound = match hop {
                PathHop::Managed {
                    endpoint,
                    relay_uuid,
                } => crate::protocols::client(
                    &endpoint.node,
                    &Access {
                        user_id: 1,
                        uuid: *relay_uuid,
                        credential: String::new(),
                    },
                ),
                PathHop::External { outbound, .. } => serde_json::to_value(outbound)?,
            };
            outbound["tag"] = json!(super::path_outbound_tag(
                path.chain_id,
                path.generation,
                index + 1
            ));
            if index > 0 {
                outbound["detour"] = json!(super::path_outbound_tag(
                    path.chain_id,
                    path.generation,
                    index
                ));
            }
            // The local bootstrap resolver has no outbound dependency, so DNS
            // cannot recurse through the path it is trying to establish.
            outbound["domain_resolver"] = json!({"server":"chain-bootstrap"});
            config["outbounds"]
                .as_array_mut()
                .expect("compiled outbounds")
                .push(outbound);
        }
        let inbound = format!("node-{}", path.entry_node_id);
        if path.active {
            let capabilities = super::path_capabilities(path)?;
            let blocked = if !capabilities.tcp {
                Some("tcp")
            } else if !capabilities.udp {
                Some("udp")
            } else {
                None
            };
            if let Some(network) = blocked {
                rules.push(json!({"inbound":[inbound],"network":[network],"action":"reject"}));
            }
            rules.push(json!({"inbound":[inbound],"action":"route","outbound":path.final_tag()}));
        } else if !active_entries.contains(&path.entry_node_id)
            && blocked_entries.insert(path.entry_node_id)
        {
            // A candidate can be probed by explicit selector without allowing
            // its public listener to fall through to the direct route.
            rules.push(json!({"inbound":[inbound],"action":"reject"}));
        }
    }
    if let Some(node_rules) = config["route"]["rules"].as_array() {
        rules.extend(node_rules.iter().cloned());
    }
    if !rules.is_empty() {
        config["route"]["rules"] = Value::Array(rules);
    }
    if !paths.is_empty() {
        if config["dns"].is_null() {
            config["dns"] = json!({"servers":[]});
        }
        config["dns"]["servers"]
            .as_array_mut()
            .ok_or_else(|| CompileError::InvalidPath {
                chain_id: paths[0].chain_id,
                position: None,
                reason: "compiled DNS inventory is invalid",
            })?
            .push(json!({"type":"local","tag":"chain-bootstrap"}));
        config["dns"]["final"] = json!("chain-bootstrap");
    }
    if let Some(probe) = probe_control {
        let expected = format!("127.0.0.1:{}", probe.listen_port);
        let existing = &config["experimental"]["clash_api"];
        if !existing.is_null()
            && (existing["external_controller"].as_str() != Some(&expected)
                || existing["secret"].as_str() != Some(probe.secret.as_str()))
        {
            return Err(CompileError::InvalidPath {
                chain_id: paths.first().map(|path| path.chain_id).unwrap_or(0),
                position: None,
                reason: "private control configuration differs between path pipelines",
            });
        }
        if existing.is_null() {
            config["experimental"]["clash_api"] =
                json!({"external_controller":expected,"secret":probe.secret});
        }
    }
    crate::pretty(config)
}

fn add_acceptance(config: &mut Value, accept: &ManagedAcceptance) -> Result<(), CompileError> {
    let node = &accept.endpoint.node;
    let tag = format!("node-{}", node.id);
    let identity =
        crate::protocols::reality_identity(node, acceptance_name(accept), accept.relay_uuid);
    let inbounds = config["inbounds"]
        .as_array_mut()
        .expect("compiled inbounds");
    if let Some(inbound) = inbounds.iter_mut().find(|inbound| inbound["tag"] == tag) {
        let mut expected = crate::protocols::server(node, &[]);
        let mut actual = inbound.clone();
        expected
            .as_object_mut()
            .expect("compiled inbound")
            .remove("users");
        actual
            .as_object_mut()
            .expect("compiled inbound")
            .remove("users");
        if expected != actual {
            return Err(CompileError::InvalidPath {
                chain_id: accept.chain_id,
                position: Some(accept.position as usize),
                reason: "managed listener differs between frozen path snapshots",
            });
        }
        if inbound["users"].as_array().is_some_and(|users| {
            users
                .iter()
                .any(|user| user["uuid"] == identity["uuid"] || user["name"] == identity["name"])
        }) {
            return Err(CompileError::InvalidPath {
                chain_id: accept.chain_id,
                position: Some(accept.position as usize),
                reason: "internal relay identity repeats another path identity",
            });
        }
        inbound["users"]
            .as_array_mut()
            .expect("compiled identities")
            .push(identity);
    } else {
        let mut inbound = crate::protocols::server(node, &[]);
        inbound["users"] = json!([identity]);
        inbounds.push(inbound);
    }
    Ok(())
}

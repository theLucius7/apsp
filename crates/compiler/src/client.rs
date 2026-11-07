//! Compose authorized external provider outbounds with applied managed nodes.
use crate::{CompileError, Node, authorized_nodes, external::ExternalOutbound, pretty};
use serde_json::json;
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct ExternalClientNode {
    pub id: i64,
    pub name: String,
    pub sort_order: i64,
    pub outbound: ExternalOutbound,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Managed(#[from] CompileError),
    #[error("external node {id}: {reason}")]
    External { id: i64, reason: &'static str },
}

/// The caller must establish external provider authorization and source identity.
/// External definitions never receive invented managed user credentials or detours.
pub fn compile_with_external(
    nodes: &[Node],
    user_id: i64,
    external: &[ExternalClientNode],
) -> Result<String, ClientError> {
    if external.is_empty() {
        return Ok(crate::compile_client(nodes, user_id)?);
    }
    let accesses = authorized_nodes(nodes, user_id)?;
    let mut external: Vec<_> = external.iter().collect();
    external.sort_by_key(|node| (node.sort_order, node.id));
    let mut ids = BTreeSet::new();
    let mut tags: Vec<_> = accesses
        .iter()
        .flat_map(|(node, access)| {
            crate::client_variants(node, access)
                .into_iter()
                .filter_map(|(_, v)| v["tag"].as_str().map(str::to_owned))
        })
        .collect();
    let mut outbounds = Vec::new();
    for (node, access) in accesses {
        outbounds.extend(
            crate::client_variants(node, access)
                .into_iter()
                .map(|(_, value)| value),
        );
    }
    for node in external {
        if node.id <= 0 || !ids.insert(node.id) {
            return Err(ClientError::External {
                id: node.id,
                reason: "invalid or duplicate identity",
            });
        }
        let name = node.name.trim();
        if name.is_empty() || name.chars().count() > 160 || node.name.chars().any(char::is_control)
        {
            return Err(ClientError::External {
                id: node.id,
                reason: "invalid display name",
            });
        }
        let tag = format!("external-node-{} {name}", node.id);
        outbounds.push(node.outbound.render(&tag, None, None).map_err(|error| {
            ClientError::External {
                id: node.id,
                reason: error.0,
            }
        })?);
        tags.push(tag);
    }
    outbounds.insert(0, json!({"type":"selector","tag":"proxy","outbounds":tags}));
    outbounds.push(json!({"type":"direct","tag":"direct"}));
    Ok(pretty(json!({
        "log":{"level":"warn","timestamp":true},
        "inbounds":[{"type":"mixed","tag":"mixed-in","listen":"127.0.0.1","listen_port":2080}],
        "outbounds":outbounds,"route":{"final":"proxy"}
    }))?)
}

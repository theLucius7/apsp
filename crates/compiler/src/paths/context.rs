use super::validate::{address, compatible, endpoint, error};
use super::{ManagedAcceptance, OrderedPath, PathHop, ProbeControl};
use crate::{CompileError, Node};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

fn failure(chain_id: i64, reason: &'static str) -> CompileError {
    CompileError::InvalidPath {
        chain_id,
        position: None,
        reason,
    }
}

pub(super) fn validate(
    nodes: &[Node],
    paths: &[OrderedPath],
    accepts: &[ManagedAcceptance],
    probe: Option<&ProbeControl>,
) -> Result<(), CompileError> {
    let first_chain = paths
        .first()
        .map(|path| path.chain_id)
        .or_else(|| accepts.first().map(|accept| accept.chain_id))
        .unwrap_or(0);
    let mut local_server = None;
    let mut observe_server = |server_id| -> Result<(), CompileError> {
        if local_server.is_some_and(|previous| previous != server_id) {
            return Err(failure(
                first_chain,
                "compiler inputs span multiple local servers",
            ));
        }
        local_server = Some(server_id);
        Ok(())
    };
    let mut versions = BTreeMap::new();
    let mut entries = BTreeMap::new();
    let mut generations = BTreeSet::new();
    let mut active = BTreeSet::new();
    let public_uuids: BTreeSet<_> = nodes
        .iter()
        .flat_map(|node| node.users.iter().map(|user| user.uuid))
        .collect();
    for path in paths {
        super::validate_path(path)?;
        observe_server(path.entry_server_id)?;
        if !generations.insert((path.chain_id, path.generation)) {
            return Err(error(path, None, "path generation appears more than once"));
        }
        if let Some(previous) = entries.insert(path.entry_node_id, path.chain_id)
            && previous != path.chain_id
        {
            return Err(error(path, None, "public entry belongs to multiple chains"));
        }
        if paths.iter().any(|previous| {
            previous.chain_id == path.chain_id
                && (previous.entry_node_id != path.entry_node_id
                    || previous.entry_server_id != path.entry_server_id)
        }) {
            return Err(error(
                path,
                None,
                "generations of a chain must retain their public entry identity",
            ));
        }
        if path.active && !active.insert(path.entry_node_id) {
            return Err(error(
                path,
                None,
                "public entry has multiple active generations",
            ));
        }
        let entry = nodes
            .iter()
            .find(|node| node.id == path.entry_node_id)
            .ok_or_else(|| {
                error(
                    path,
                    None,
                    "public entry is absent from the local node snapshot",
                )
            })?;
        if !entry.enabled || !entry.protocol_config.is_reality() {
            return Err(error(
                path,
                None,
                "public entry must be an enabled Reality node",
            ));
        }
        let entry_address = address(&entry.public_host, entry.public_port());
        for (position, hop) in path.hops.iter().enumerate() {
            let host_port = match hop {
                PathHop::Managed {
                    endpoint: snapshot,
                    relay_uuid,
                } => {
                    if public_uuids.contains(relay_uuid) {
                        return Err(error(
                            path,
                            Some(position + 1),
                            "internal relay identity collides with a public grant",
                        ));
                    }
                    if nodes.iter().any(|node| node.id == snapshot.node.id) {
                        return Err(error(
                            path,
                            Some(position + 1),
                            "managed hop is local to the entry compiler snapshot",
                        ));
                    }
                    if let Some(previous) = versions.insert(snapshot.version_id, snapshot.as_ref())
                        && (previous.server_id != snapshot.server_id
                            || !compatible(&previous.node, &snapshot.node)?)
                    {
                        return Err(error(
                            path,
                            Some(position + 1),
                            "frozen endpoint version has inconsistent content",
                        ));
                    }
                    address(&snapshot.node.public_host, snapshot.node.public_port())
                }
                PathHop::External { outbound, .. } => {
                    address(&outbound.common().server, outbound.common().server_port)
                }
            };
            if host_port == entry_address {
                return Err(error(
                    path,
                    Some(position + 1),
                    "hop endpoint loops back to the public entry address",
                ));
            }
        }
    }
    for path in paths {
        for hop in &path.hops {
            if let PathHop::Managed { endpoint, .. } = hop
                && entries.contains_key(&endpoint.node.id)
            {
                return Err(error(
                    path,
                    None,
                    "another public chain entry cannot be used as a managed hop",
                ));
            }
        }
    }
    let mut acceptance_ids = BTreeSet::new();
    let mut relay_uuids: BTreeSet<Uuid> = BTreeSet::new();
    let mut listeners: BTreeMap<i64, &Node> = nodes.iter().map(|node| (node.id, node)).collect();
    let mut ports: BTreeMap<u16, i64> = nodes.iter().map(|node| (node.port, node.id)).collect();
    for accept in accepts {
        endpoint(&accept.endpoint).map_err(|reason| failure(accept.chain_id, reason))?;
        observe_server(accept.endpoint.server_id)?;
        if accept.chain_id <= 0
            || accept.generation == 0
            || !(1..=8).contains(&accept.position)
            || accept.relay_uuid.is_nil()
            || !acceptance_ids.insert((accept.chain_id, accept.generation, accept.position))
        {
            return Err(failure(
                accept.chain_id,
                "internal acceptance requires a unique frozen position and identity",
            ));
        }
        if entries.contains_key(&accept.endpoint.node.id) {
            return Err(failure(
                accept.chain_id,
                "public entry cannot also accept an internal relay",
            ));
        }
        if !relay_uuids.insert(accept.relay_uuid) || public_uuids.contains(&accept.relay_uuid) {
            return Err(failure(
                accept.chain_id,
                "internal acceptance identity collides with an existing identity",
            ));
        }
        if let Some(previous) = versions.insert(accept.endpoint.version_id, &accept.endpoint)
            && (previous.server_id != accept.endpoint.server_id
                || !compatible(&previous.node, &accept.endpoint.node)?)
        {
            return Err(failure(
                accept.chain_id,
                "frozen endpoint version has inconsistent content",
            ));
        }
        let node = &accept.endpoint.node;
        if let Some(previous) = listeners.insert(node.id, node)
            && !compatible(previous, node)?
        {
            return Err(failure(
                accept.chain_id,
                "frozen listener cannot coexist with current endpoint parameters",
            ));
        }
        if node.port == 18085
            || ports
                .insert(node.port, node.id)
                .is_some_and(|previous| previous != node.id)
        {
            return Err(failure(
                accept.chain_id,
                "internal listener conflicts with another local listener",
            ));
        }
    }
    if let Some(probe) = probe {
        if probe.listen_port == 0
            || probe.listen_port == 18085
            || probe.secret.len() != 64
            || !probe.secret.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(failure(
                first_chain,
                "probe controller requires a dedicated port and private 64-digit secret",
            ));
        }
        if ports.contains_key(&probe.listen_port) {
            return Err(failure(
                first_chain,
                "probe controller conflicts with a proxy listener",
            ));
        }
    }
    Ok(())
}

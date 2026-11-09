use crate::error::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub format: String,
    pub source_id: String,
    pub infrastructure_digest: String,
    pub exported_at: i64,
    pub hosts: Vec<Host>,
    pub machines: Vec<Machine>,
    pub groups: Vec<Value>,
    pub routes: Vec<Value>,
    pub nodes: Vec<Value>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Host {
    pub legacy_id: i64,
    pub server_id: i64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Machine {
    pub id: i64,
    pub host_id: i64,
    pub name: String,
    pub is_active: i32,
}

pub(super) struct PreparedNode {
    pub legacy_id: i64,
    pub machine_id: i64,
    pub host_id: i64,
    pub server_id: i64,
    pub display: Value,
    pub archive: Value,
}

pub(super) fn bad() -> ApiError {
    ApiError::BadRequest("节点快照的格式、数量或引用关系无效".into())
}

pub(super) fn digest(snapshot: &Snapshot) -> ApiResult<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(snapshot).map_err(anyhow::Error::from)?)
    ))
}

pub(super) fn source_tag(source: &str, host: i64) -> String {
    format!(
        "nexus:{}:{host}",
        &format!("{:x}", Sha256::digest(source.as_bytes()))[..12]
    )
}

fn identifier(id: i64) -> bool {
    (1..=crate::business::MAX_SAFE_INTEGER).contains(&id)
}

fn id(row: &Value) -> ApiResult<i64> {
    row["id"]
        .as_i64()
        .filter(|id| identifier(*id))
        .ok_or_else(bad)
}

fn text(value: &Value, limit: usize, empty: bool) -> ApiResult<&str> {
    value
        .as_str()
        .filter(|s| {
            (empty || !s.trim().is_empty()) && s.len() <= limit && !s.chars().any(char::is_control)
        })
        .ok_or_else(bad)
}

fn decoded(value: &Value, array: bool) -> ApiResult<Value> {
    let value = match value {
        Value::Null => {
            if array {
                json!([])
            } else {
                json!({})
            }
        }
        Value::String(value) => serde_json::from_str(value).map_err(|_| bad())?,
        value => value.clone(),
    };
    if (array && !value.is_array()) || (!array && !value.is_object()) {
        return Err(bad());
    }
    Ok(value)
}

fn references(value: &Value) -> ApiResult<Vec<i64>> {
    let value = decoded(value, true)?;
    let ids = value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                .filter(|id| identifier(*id))
                .ok_or_else(bad)
        })
        .collect::<ApiResult<Vec<_>>>()?;
    if ids.len() > 1000 || ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
        return Err(bad());
    }
    Ok(ids)
}

fn flag(value: &Value) -> ApiResult<bool> {
    match value {
        Value::Bool(value) => Ok(*value),
        _ => match value.as_i64() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(bad()),
        },
    }
}

fn port(value: &Value, range: bool) -> ApiResult<String> {
    let value = match value {
        Value::String(s) => s.clone(),
        _ => value.as_u64().ok_or_else(bad)?.to_string(),
    };
    let ports = value
        .split('-')
        .map(|s| s.parse::<u16>().ok().filter(|p| *p > 0))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(bad)?;
    if !(ports.len() == 1 || range && ports.len() == 2 && ports[0] <= ports[1]) {
        return Err(bad());
    }
    Ok(value)
}

fn indexed(rows: &[Value]) -> ApiResult<BTreeMap<i64, &Value>> {
    let mut result = BTreeMap::new();
    for row in rows {
        if result.insert(id(row)?, row).is_some() {
            return Err(bad());
        }
    }
    Ok(result)
}

// Only explicit public scalar fields are copied. Unknown nested fields, TLS
// keys, obfuscation passwords and outbound credentials stay in the archive.
fn protocol_summary(protocol: &Value) -> Value {
    let mut out = json!({});
    for (name, path) in [
        ("network", "/network"),
        ("flow", "/flow"),
        ("tls", "/tls"),
        ("reality_sni", "/reality_settings/server_name"),
        ("reality_port", "/reality_settings/server_port"),
        ("public_key", "/reality_settings/public_key"),
        ("short_id", "/reality_settings/short_id"),
        ("tls_sni", "/tls/server_name"),
        ("tls_settings_sni", "/tls_settings/server_name"),
        ("cipher", "/cipher"),
        ("version", "/version"),
        ("hop_interval", "/hop_interval"),
        ("multiplex", "/multiplex/enabled"),
        ("multiplex_protocol", "/multiplex/protocol"),
        ("encryption_enabled", "/encryption/enabled"),
        ("fingerprint", "/utls/fingerprint"),
        ("obfs", "/obfs/open"),
        ("allow_insecure", "/tls/allow_insecure"),
    ] {
        if let Some(value) = protocol.pointer(path).filter(|v| {
            v.is_boolean()
                || v.is_number()
                || v.as_str()
                    .is_some_and(|s| s.len() <= 2048 && !s.chars().any(char::is_control))
        }) {
            out[name] = value.clone();
        }
    }
    out
}

pub(super) fn prepare(snapshot: &Snapshot) -> ApiResult<Vec<PreparedNode>> {
    if snapshot.format != "apsp.nexus.nodes.v1"
        || snapshot.source_id.is_empty()
        || snapshot.source_id.len() > 64
        || !snapshot
            .source_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        || snapshot.infrastructure_digest.len() != 64
        || !snapshot
            .infrastructure_digest
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
        || snapshot.exported_at <= 0
        || snapshot.nodes.is_empty()
        || snapshot.nodes.len() > 5000
        || snapshot.hosts.len() > 5000
        || snapshot.machines.len() > 10000
        || snapshot.groups.len() > 1000
        || snapshot.routes.len() > 1000
    {
        return Err(bad());
    }
    let mut hosts = BTreeMap::new();
    let mut servers = BTreeSet::new();
    for host in &snapshot.hosts {
        if !identifier(host.legacy_id)
            || !identifier(host.server_id)
            || hosts.insert(host.legacy_id, host).is_some()
            || !servers.insert(host.server_id)
        {
            return Err(bad());
        }
    }
    let mut machines = BTreeMap::new();
    for machine in &snapshot.machines {
        crate::business::name(&machine.name)?;
        if !identifier(machine.id)
            || !hosts.contains_key(&machine.host_id)
            || ![0, 1].contains(&machine.is_active)
            || machines.insert(machine.id, machine).is_some()
        {
            return Err(bad());
        }
    }
    let groups = indexed(&snapshot.groups)?;
    let routes = indexed(&snapshot.routes)?;
    let mut ids = BTreeSet::new();
    let mut prepared = Vec::new();
    for node in &snapshot.nodes {
        let legacy_id = id(node)?;
        if !ids.insert(legacy_id) {
            return Err(bad());
        }
        let name = text(&node["name"], 512, false)?;
        let protocol = text(&node["type"], 40, false)?;
        let endpoint = text(&node["host"], 253, false)?;
        if endpoint
            .chars()
            .any(|c| c.is_whitespace() || "/@?#".contains(c))
        {
            return Err(bad());
        }
        let machine = machines
            .get(&node["machine_id"].as_i64().ok_or_else(bad)?)
            .ok_or_else(bad)?;
        let host = hosts[&machine.host_id];
        let group_ids = references(&node["group_ids"])?;
        let route_ids = references(&node["route_ids"])?;
        let node_groups = group_ids
            .iter()
            .map(|id| groups.get(id).copied().ok_or_else(bad))
            .collect::<ApiResult<Vec<_>>>()?;
        let node_routes = route_ids
            .iter()
            .map(|id| routes.get(id).copied().ok_or_else(bad))
            .collect::<ApiResult<Vec<_>>>()?;
        let group_views = node_groups
            .iter()
            .map(|g| Ok(json!({"id": id(g)?, "name":text(&g["name"],512,false)?})))
            .collect::<ApiResult<Vec<_>>>()?;
        let route_views = node_routes
            .iter()
            .map(|r| Ok(json!({"id":id(r)?,"name":text(&r["remarks"],512,true)?})))
            .collect::<ApiResult<Vec<_>>>()?;
        let settings = decoded(&node["protocol_settings"], false)?;
        let tags = decoded(&node["tags"], true)?;
        if tags.as_array().unwrap().len() > 100 {
            return Err(bad());
        }
        for tag in tags.as_array().unwrap() {
            text(tag, 128, false)?;
        }
        let rate = if node["rate"].is_number() {
            node["rate"].to_string()
        } else {
            text(&node["rate"], 30, false)?.to_owned()
        };
        if !rate
            .parse::<f64>()
            .is_ok_and(|v| v.is_finite() && (0.0..=1_000_000.0).contains(&v))
        {
            return Err(bad());
        }
        let parent = node["parent_id"].as_i64().filter(|id| *id > 0);
        let display = json!({
            "name":name, "protocol":protocol, "public_host":endpoint,
            "public_port":port(&node["port"],true)?, "listen_port":port(&node["server_port"],false)?,
            "rate":rate, "enabled":flag(&node["enabled"])?, "visible":flag(&node["show"])?,
            "sort_order":node["sort"].as_i64().unwrap_or(0), "tags":tags,
            "parent_id":parent, "groups":group_views, "routes":route_views,
            "machine_name":machine.name, "machine_active":machine.is_active == 1,
            "protocol_summary":protocol_summary(&settings),
            "custom_route_count":decoded(&node["custom_routes"],true)?.as_array().unwrap().len(),
            "custom_outbound_count":decoded(&node["custom_outbounds"],true)?.as_array().unwrap().len(),
            "timed_rate_enabled":flag(&node["rate_time_enable"])?,
            "timed_rate_count":decoded(&node["rate_time_ranges"],true)?.as_array().unwrap().len(),
            "certificate_saved":!decoded(&node["cert_config"],false)?.as_object().unwrap().is_empty(),
            "configuration_saved":true, "state":"pending_agent"
        });
        let archive = json!({"source_id":snapshot.source_id,"infrastructure_digest":snapshot.infrastructure_digest,
            "exported_at":snapshot.exported_at,"host":host,"machine":machine,"node":node,"groups":node_groups,"routes":node_routes});
        if serde_json::to_vec(&archive)
            .map_err(anyhow::Error::from)?
            .len()
            > 65536
        {
            return Err(bad());
        }
        prepared.push(PreparedNode {
            legacy_id,
            machine_id: machine.id,
            host_id: host.legacy_id,
            server_id: host.server_id,
            display,
            archive,
        });
    }
    Ok(prepared)
}

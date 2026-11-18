//! Member line catalogue and probe panel (ADR 0106).
//!
//! A signed-in member sees the lines of the migrated Nexus catalogue: how each
//! route is built, whether their policy groups include it, its status, multiplier and
//! carrier latency, and for every host the network attributes measured by the
//! panel's IP quality datasets. Endpoint addresses, SNI and key material,
//! monitor identifiers, cities and coordinates, and other members never appear.
use super::access::{lock, session, session_hash};
use crate::AppState;
use crate::error::ApiError;
use crate::error::ApiResult;
use apsp_panel_host::ports::{BoxFuture, Port};
use apsp_protocol::now_timestamp;
use apsp_traffic::nexus_nodes::topology;
use apsp_traffic::nexus_nodes::topology::CatalogNode;
use apsp_traffic::nexus_nodes::topology::Topology;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde_json::{Value, json};
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// Devices that reported within this window are online.
const ONLINE_SECS: i64 = 60;
/// Latency window the catalogue summarizes.
const LATENCY_WINDOW_MS: i64 = 3_600_000;

struct Line {
    legacy_id: i64,
    server_id: i64,
    display: Value,
    tags: Vec<String>,
    taken_over: bool,
    /// The native node running a taken-over line, served by policy groups.
    runtime_node: Option<i64>,
}

struct Host {
    server_id: i64,
    agent_seen: Option<i64>,
    metrics: Value,
    static_info: Value,
    runtime_healthy: bool,
}

/// Protocol name, transport and security only; no SNI, key or short id.
fn protocol(display: &Value) -> Value {
    let name = display["protocol"]
        .as_str()
        .unwrap_or("unknown")
        .to_lowercase();
    let summary = &display["protocol_summary"];
    let quic = matches!(name.as_str(), "hysteria" | "hysteria2" | "tuic");
    let network = summary["network"]
        .as_str()
        .filter(|value| value.len() <= 16 && value.bytes().all(|b| b.is_ascii_alphanumeric()))
        .map(str::to_owned)
        .unwrap_or_else(|| if quic { "udp" } else { "tcp" }.into());
    let tls = summary["tls"].as_i64();
    let security = if tls == Some(2) || summary.get("reality_sni").is_some() {
        "reality"
    } else if tls == Some(1) || quic || summary.get("tls_sni").is_some() {
        "tls"
    } else {
        "none"
    };
    json!({"name":name,"network":network,"security":security})
}

fn percent(used: &Value, total: &Value) -> Value {
    match (used.as_f64(), total.as_f64()) {
        (Some(used), Some(total)) if total > 0.0 => {
            json!(((used / total * 1000.0).round() / 10.0).clamp(0.0, 100.0))
        }
        _ => Value::Null,
    }
}

fn rounded(value: &Value) -> Value {
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .map(|value| json!((value * 10.0).round() / 10.0))
        .unwrap_or(Value::Null)
}

/// The network attributes and carrier probe latencies a member may see for a host,
/// measured by observability without the addresses, monitor identifiers, cities,
/// coordinates or time zones. Observability answers and the application connects it.
pub trait HostNetwork: Send + Sync {
    fn member_network<'a>(
        &'a self,
        state: &'a AppState,
        server: i64,
        static_info: &'a Value,
    ) -> BoxFuture<'a, ApiResult<Value>>;

    fn carrier_latencies<'a>(
        &'a self,
        connection: &'a mut sqlx::PgConnection,
        servers: &'a [i64],
        since_ms: i64,
    ) -> BoxFuture<'a, ApiResult<BTreeMap<i64, Value>>>;
}

static HOST_NETWORK: Port<dyn HostNetwork> = Port::new();

pub fn install(network: &'static dyn HostNetwork) {
    HOST_NETWORK.install(network);
}

async fn member_network(state: &AppState, server: i64, static_info: &Value) -> ApiResult<Value> {
    match HOST_NETWORK.get() {
        Some(network) => network.member_network(state, server, static_info).await,
        None => Err(ApiError::Conflict(
            "线路网络属性的提供方未登记，不能显示主机的网络属性".into(),
        )),
    }
}

async fn carrier_latencies(
    connection: &mut sqlx::PgConnection,
    servers: &[i64],
    since_ms: i64,
) -> ApiResult<BTreeMap<i64, Value>> {
    match HOST_NETWORK.get() {
        Some(network) => {
            network
                .carrier_latencies(connection, servers, since_ms)
                .await
        }
        None => Err(ApiError::Conflict(
            "线路网络属性的提供方未登记，不能显示主机的网络属性".into(),
        )),
    }
}

fn legacy_latency(status: &Value) -> Option<Value> {
    let latency = &status["latency"];
    (latency["monitored"] == true).then(|| {
        json!({"source":"legacy_monitor","monitored":true,"updated_at":latency["updated_at"],
            "window_minutes":60,"direction":"node_to_carrier","carriers":latency["carriers"]})
    })
}

pub(super) async fn view(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    let owner = lock(&mut tx, account).await?;
    session(&mut tx, account, &session_hash(&headers)?, false).await?;
    let user: i64 = owner.try_get("user_id")?;
    let now = now_timestamp();
    let package: Option<i64> = sqlx::query_scalar(
        "SELECT package_group_id FROM singbox_entitlements($1) WHERE user_id=$2 AND allowed LIMIT 1",
    )
    .bind(now)
    .bind(user)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    // Which lines the member may use follows policy groups (ADR 0115): the old
    // panel lists the lines of their one original group, and a taken-over line
    // also needs a policy group that holds it.
    let allowed: bool = sqlx::query_scalar(
        "SELECT COALESCE(bool_or(allowed),FALSE) FROM singbox_entitlements($1) WHERE user_id=$2",
    )
    .bind(now)
    .bind(user)
    .fetch_one(&mut *tx)
    .await?;
    let original: Option<i64> = sqlx::query_scalar("SELECT singbox_member_legacy_group($1)")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    let served: BTreeSet<i64> = sqlx::query_scalar::<_, i64>("SELECT DISTINCT pn.node_id FROM singbox_user_policies up JOIN singbox_policy_nodes pn ON pn.group_id=up.group_id WHERE up.user_id=$1")
        .bind(user)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();
    // Users the bridge does not serve use a line apsp runs with the ordinary credential
    // published for them (ADR 0138); the original software knows only members.
    let legacy_served: bool = sqlx::query_scalar("SELECT singbox_legacy_served($1)")
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    let ordinary: BTreeSet<i64> = if legacy_served {
        BTreeSet::new()
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT node_id FROM singbox_eligible_accesses($1) WHERE user_id=$2",
        )
        .bind(now)
        .bind(user)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect()
    };
    let mappings = crate::nexus::bridge::plan_mappings(&mut tx).await?;
    let rows = sqlx::query("SELECT n.legacy_node_id,n.server_id,n.display,(r.node_id IS NOT NULL AND h.state='active') AS taken_over,r.node_id AS runtime_node FROM singbox_nexus_nodes n LEFT JOIN singbox_nexus_runtime_nodes r ON r.inventory_id=n.id LEFT JOIN singbox_nexus_runtime_hosts h ON h.server_id=r.server_id WHERE COALESCE((n.display->>'visible')::BOOLEAN,FALSE) ORDER BY COALESCE((n.display->>'sort_order')::BIGINT,0),n.legacy_node_id LIMIT 2000")
        .fetch_all(&mut *tx).await?;
    let lines: Vec<Line> = rows
        .iter()
        .map(|row| {
            let display: Value = row.try_get("display")?;
            let tags = display["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|tag| tag.as_str().map(str::to_owned))
                .collect();
            Ok(Line {
                legacy_id: row.try_get("legacy_node_id")?,
                server_id: row.try_get("server_id")?,
                display,
                tags,
                taken_over: row
                    .try_get::<Option<bool>, _>("taken_over")?
                    .unwrap_or(false),
                runtime_node: row.try_get("runtime_node")?,
            })
        })
        .collect::<Result<_, sqlx::Error>>()?;
    let legacy: BTreeMap<i64, Value> = sqlx::query_as::<_, (i64, Value)>("SELECT legacy_node_id,jsonb_build_object('online',online,'checked_at',checked_at,'load',load,'latency',latency,'observed_at',observed_at) FROM singbox_nexus_node_status WHERE observed_at>=$1")
        .bind(now - 300).fetch_all(&mut *tx).await?.into_iter().collect();
    let server_ids: Vec<i64> = lines
        .iter()
        .map(|line| line.server_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut metrics = crate::server_observation::stored_metrics(&mut tx, &server_ids).await?;
    let hosts: BTreeMap<i64, Host> = sqlx::query("SELECT s.id,s.last_seen,COALESCE(s.static_info,'{}'::JSONB) AS info,COALESCE(m.healthy AND m.applied_rev=m.target_rev,FALSE) AS healthy FROM servers s LEFT JOIN server_module_status m ON m.server_id=s.id AND m.module='singbox' WHERE s.id=ANY($1) AND s.deleted_at IS NULL")
        .bind(&server_ids).fetch_all(&mut *tx).await?
        .iter()
        .map(|row| {
            let id: i64 = row.try_get("id")?;
            Ok((id, Host {
                server_id: id,
                agent_seen: row.try_get("last_seen")?,
                metrics: metrics.remove(&id).unwrap_or_default().metrics,
                static_info: row.try_get("info")?,
                runtime_healthy: row.try_get("healthy")?,
            }))
        })
        .collect::<Result<_, sqlx::Error>>()?;
    let mut latencies = carrier_latencies(
        &mut tx,
        &server_ids,
        apsp_protocol::telemetry::now_millis() - LATENCY_WINDOW_MS,
    )
    .await?;
    tx.commit().await?;

    // Topology: shared-ingress inference only from direct lines with status.
    let catalog: Vec<CatalogNode<'_>> = lines
        .iter()
        .map(|line| CatalogNode {
            id: line.legacy_id,
            name: line.display["name"].as_str().unwrap_or(""),
            tags: &line.tags,
            host: line.display["public_host"].as_str().unwrap_or(""),
        })
        .collect();
    let ingress: Vec<i64> = catalog
        .iter()
        .zip(&lines)
        .filter(|(node, line)| {
            topology::direct(node)
                && (legacy.contains_key(&line.legacy_id)
                    || hosts
                        .get(&line.server_id)
                        .is_some_and(|h| h.agent_seen.is_some()))
        })
        .map(|(node, _)| node.id)
        .collect();
    let topologies: Vec<Topology> = catalog
        .iter()
        .map(|node| topology::describe(node, &catalog, &ingress))
        .collect();
    let by_legacy: BTreeMap<i64, usize> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| (line.legacy_id, index))
        .collect();

    // Host cards, keyed in catalogue order so internal server ids stay private.
    let mut keys: BTreeMap<i64, String> = BTreeMap::new();
    for line in &lines {
        let next = format!("h{}", keys.len() + 1);
        keys.entry(line.server_id).or_insert(next);
    }
    let mut host_latency: BTreeMap<i64, Value> = BTreeMap::new();
    let mut host_cards = Vec::new();
    for (server, key) in &keys {
        let host = hosts.get(server);
        let direct: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(index, line)| {
                line.server_id == *server && topologies[*index].kind == "direct"
            })
            .map(|(index, _)| index)
            .collect();
        let statuses: Vec<&Value> = direct
            .iter()
            .filter_map(|index| legacy.get(&lines[*index].legacy_id))
            .collect();
        let agent = host.and_then(|host| host.agent_seen).is_some();
        let (online, checked_at, load, source) = if let Some(host) = host.filter(|_| agent) {
            let seen = host.agent_seen.unwrap_or(0);
            let metrics = &host.metrics;
            (
                now - seen <= ONLINE_SECS,
                Some(seen),
                json!({"cpu":rounded(&metrics["cpu_percent"]),
                    "memory":percent(&metrics["memory_used"],&metrics["memory_total"]),
                    "disk":percent(&metrics["disk_used"],&metrics["disk_total"])}),
                "apsp",
            )
        } else if let Some(status) = statuses
            .iter()
            .find(|status| status["online"] == true)
            .or(statuses.first())
        {
            (
                status["online"] == true,
                status["checked_at"].as_i64(),
                status["load"].clone(),
                "legacy",
            )
        } else {
            (false, None, json!({}), "unknown")
        };
        let latency = latencies
            .remove(server)
            .or_else(|| statuses.iter().find_map(|status| legacy_latency(status)))
            .unwrap_or_else(|| json!({"monitored":false}));
        host_latency.insert(*server, latency.clone());
        let network = match host.filter(|_| agent) {
            Some(host) => member_network(&state, host.server_id, &host.static_info).await?,
            None => json!({"available":false,"reason":"no_agent"}),
        };
        let label = direct
            .iter()
            .map(|index| &topologies[*index].identity)
            .find(|identity| identity.region.is_some() || identity.provider.is_some());
        host_cards.push(json!({
            "key":key,
            "region":label.and_then(|identity| identity.region.clone()),
            "provider":label.and_then(|identity| identity.provider.clone()),
            "residential_declared":direct.iter().any(|index| topologies[*index].identity.residential),
            "online":online,"checked_at":checked_at,"source":source,"load":load,
            "uptime_secs":host.filter(|_| agent).and_then(|host| host.metrics["uptime_secs"].as_u64()),
            "latency":latency,"network":network,
            "lines":lines.iter().filter(|line| line.server_id == *server).map(|line| line.legacy_id).collect::<Vec<_>>(),
        }));
    }

    let mut entries = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let topology = &topologies[index];
        let groups: BTreeSet<i64> = line.display["groups"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|group| group["id"].as_i64())
            .collect();
        let mut plans = BTreeMap::new();
        for (legacy_group, package_group, name) in &mappings {
            if groups.contains(legacy_group) {
                plans.insert(
                    *package_group,
                    json!({"name":name,"current":package == Some(*package_group)}),
                );
            }
        }
        let access = if legacy_served {
            allowed
                && original.is_some_and(|group| groups.contains(&group))
                && (!line.taken_over
                    || line.runtime_node.is_some_and(|node| served.contains(&node)))
        } else {
            allowed
                && line.taken_over
                && line
                    .runtime_node
                    .is_some_and(|node| ordinary.contains(&node))
        };
        let enabled = line.display["enabled"] == true;
        let host = hosts.get(&line.server_id);
        let (online, checked_at, source) = if let Some(host) = host.filter(|_| line.taken_over) {
            let seen = host.agent_seen.unwrap_or(0);
            (
                Some(now - seen <= ONLINE_SECS && host.runtime_healthy),
                host.agent_seen,
                "apsp",
            )
        } else if line.taken_over {
            (None, None, "unknown")
        } else if let Some(status) = legacy.get(&line.legacy_id) {
            (
                Some(status["online"] == true),
                status["checked_at"].as_i64(),
                "legacy",
            )
        } else {
            (None, None, "unknown")
        };
        let state = match (enabled, online) {
            (false, _) => "offline",
            (true, None) => "unknown",
            (true, Some(false)) => "offline",
            (true, Some(true)) if topology.kind == "direct" => "online",
            (true, Some(true)) => "entry_online",
        };
        // Relays and policies run on their entry; the catalogue shows its latency.
        let reference = topology
            .front_node_id
            .and_then(|front| by_legacy.get(&front))
            .map(|front| lines[*front].server_id);
        let latency = match reference {
            Some(server) => {
                let mut latency = host_latency
                    .get(&server)
                    .cloned()
                    .unwrap_or_else(|| json!({"monitored":false}));
                latency["referenced"] = json!(true);
                latency
            }
            None => host_latency
                .get(&line.server_id)
                .cloned()
                .unwrap_or_else(|| json!({"monitored":false})),
        };
        entries.push(json!({
            "id":line.legacy_id,
            "name":line.display["name"],
            "protocol":protocol(&line.display),
            "topology":topology,
            "status":{"state":state,"checked_at":checked_at,"source":source},
            "rate":line.display["rate"],
            "timed_rate":line.display["timed_rate_enabled"] == true,
            "latency":latency,
            "host":keys.get(&reference.unwrap_or(line.server_id)),
            "network_scope":if topology.kind == "direct" {"entry"} else {"egress_unverified"},
            "plans":plans.into_values().collect::<Vec<_>>(),
            "access":access,
        }));
    }
    Ok(Json(json!({
        "generated_at":now,
        "package":package.and_then(|package| mappings.iter().find(|(_, id, _)| *id == package).map(|(_, _, name)| name.clone())),
        "lines":entries,
        "hosts":host_cards,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_summaries_never_carry_sni_keys_or_short_ids() {
        let display = json!({"protocol":"vless","protocol_summary":{"network":"tcp","tls":2,"reality_sni":"example.invalid","public_key":"TEST_ONLY","short_id":"aa"}});
        let value = protocol(&display);
        assert_eq!(
            value,
            json!({"name":"vless","network":"tcp","security":"reality"})
        );
        assert_eq!(
            protocol(&json!({"protocol":"hysteria","protocol_summary":{}}))["network"],
            "udp"
        );
    }
}

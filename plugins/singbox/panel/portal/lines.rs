//! Member line catalogue and probe panel (ADR 0100).
//!
//! A signed-in member sees the lines of the migrated Nexus catalogue: how each
//! route is built, whether it is in their package, its status, multiplier and
//! carrier latency, and for every host the network attributes measured by the
//! panel's IP quality datasets. Endpoint addresses, SNI and key material,
//! monitor identifiers, cities and coordinates, and other members never appear.
use super::access::{lock, session, session_hash};
use crate::{
    AppState,
    error::ApiResult,
    ip_quality,
    nexus_nodes::topology::{self, CatalogNode, Topology},
};
use apsp_protocol::now_timestamp;
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
}

struct Host {
    server_id: i64,
    agent_seen: Option<i64>,
    metrics: Value,
    static_info: Value,
    runtime_healthy: bool,
}

/// Maps a probe's carrier label onto the three carriers the catalogue shows.
pub(crate) fn carrier(label: &str) -> Option<&'static str> {
    let label = label.trim().to_lowercase();
    if label.contains("电信") || label.contains("telecom") || label.contains("chinanet") {
        Some("telecom")
    } else if label.contains("联通") || label.contains("unicom") {
        Some("unicom")
    } else if label.contains("移动") || label.contains("mobile") || label.contains("cmcc") {
        Some("mobile")
    } else {
        None
    }
}

/// A probe's carrier: its monitored network, else its free-form carrier label.
fn probe_carrier(spec: &Value) -> Option<&'static str> {
    match spec["monitor"]["network"].as_str() {
        Some("telecom") => Some("telecom"),
        Some("unicom") => Some("unicom"),
        Some("mobile") => Some("mobile"),
        _ => spec["carrier"].as_str().and_then(carrier),
    }
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

/// Field labels a member may see: network identity, usage class, risk flags
/// and scores. Cities, coordinates and time zones could locate a host.
fn member_field(field: &ip_quality::QualityField) -> bool {
    use ip_quality::QualityFieldKind as Kind;
    match field.kind {
        Some(Kind::Latitude | Kind::Longitude) => false,
        Some(Kind::Asn | Kind::CountryCode | Kind::Boolean | Kind::Score) => true,
        _ => matches!(
            field.label.as_str(),
            "ASN"
                | "国家代码"
                | "ISP"
                | "网络组织"
                | "用途类型"
                | "连接类型"
                | "ASN 类型"
                | "ASN 用途"
                | "组织类型"
                | "国家或地区"
        ),
    }
}

/// Measured network attributes per address family, without the address.
fn network(quality: &[ip_quality::IpQuality]) -> Value {
    let mut families: BTreeMap<&'static str, Vec<Value>> = BTreeMap::new();
    let mut summary: BTreeMap<&'static str, BTreeMap<&'static str, Value>> = BTreeMap::new();
    let mut checked = None::<i64>;
    for entry in quality {
        let Ok(address) = entry.ip.parse::<std::net::IpAddr>() else {
            continue;
        };
        if !ip_quality::public_ip(address) {
            continue;
        }
        let family = if address.is_ipv4() { "IPv4" } else { "IPv6" };
        for database in &entry.databases {
            let fields: Vec<Value> = database
                .fields
                .iter()
                .filter(|field| member_field(field) && !field.value.is_null())
                .map(|field| json!({"label":field.label,"value":field.value,"kind":field.kind}))
                .collect();
            if fields.is_empty() {
                continue;
            }
            let found = summary.entry(family).or_default();
            for field in &database.fields {
                let key = match (field.kind, field.label.as_str()) {
                    (Some(ip_quality::QualityFieldKind::Asn), _) | (None, "ASN") => "asn",
                    (Some(ip_quality::QualityFieldKind::CountryCode), _) | (None, "国家代码") => {
                        "country"
                    }
                    (_, "网络组织" | "ISP") => "organization",
                    _ => continue,
                };
                if !field.value.is_null() {
                    found.entry(key).or_insert_with(|| field.value.clone());
                }
            }
            let at = database.last_success_at.or(database.attempted_at);
            checked = checked.max(at);
            families.entry(family).or_default().push(json!({
                "source":database.label,"database":database.database,"checked_at":at,"historical":database.historical,"fields":fields
            }));
        }
    }
    if families.is_empty() {
        return json!({"available":false});
    }
    json!({"available":true,"checked_at":checked,"families":families.into_iter().map(|(family, datasets)| json!({
        "family":family,"summary":summary.remove(family).unwrap_or_default(),"datasets":datasets
    })).collect::<Vec<_>>()})
}

/// Latest, one-hour average and loss per carrier from the host's own probes.
fn probe_latency(rows: &[(Value, Value)]) -> Option<Value> {
    let mut carriers = serde_json::Map::new();
    let mut updated = None::<i64>;
    for (spec, results) in rows {
        let Some(carrier) = probe_carrier(spec) else {
            continue;
        };
        let results = results.as_array().cloned().unwrap_or_default();
        if results.is_empty() || carriers.contains_key(carrier) {
            continue;
        }
        let latencies: Vec<f64> = results
            .iter()
            .filter_map(|result| result["latency_ms"].as_f64())
            .collect();
        let losses: Vec<f64> = results
            .iter()
            .filter_map(|result| result["loss_percent"].as_f64())
            .collect();
        let average = |values: &[f64]| {
            (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
        };
        updated = updated.max(results[0]["sampled_at"].as_i64().map(|ms| ms / 1000));
        carriers.insert(
            carrier.into(),
            json!({
                "latest":results[0]["latency_ms"].as_f64().map(f64::round),
                "average":average(&latencies).map(f64::round),
                "loss":average(&losses).map(|loss| (loss * 10.0).round() / 10.0),
            }),
        );
    }
    (!carriers.is_empty()).then(|| {
        json!({"source":"apsp_probe","monitored":true,"updated_at":updated,"window_minutes":60,
            "direction":"node_to_carrier","carriers":carriers})
    })
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
    let mappings: Vec<(i64, i64, String)> = sqlx::query_as("SELECT p.legacy_group_id,p.package_group_id,g.name FROM singbox_nexus_plan_mappings p JOIN singbox_nexus_bridge b ON b.import_id=p.import_id AND b.enabled JOIN singbox_package_groups g ON g.id=p.package_group_id WHERE p.legacy_group_id IS NOT NULL ORDER BY g.name,g.id")
        .fetch_all(&mut *tx).await?;
    let rows = sqlx::query("SELECT n.legacy_node_id,n.server_id,n.display,(r.node_id IS NOT NULL AND h.state='active') AS taken_over FROM singbox_nexus_nodes n LEFT JOIN singbox_nexus_runtime_nodes r ON r.inventory_id=n.id LEFT JOIN singbox_nexus_runtime_hosts h ON h.server_id=r.server_id WHERE COALESCE((n.display->>'visible')::BOOLEAN,FALSE) ORDER BY COALESCE((n.display->>'sort_order')::BIGINT,0),n.legacy_node_id LIMIT 2000")
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
    let hosts: BTreeMap<i64, Host> = sqlx::query("SELECT s.id,s.last_seen,COALESCE(s.latest_metrics,'{}'::JSONB) AS metrics,COALESCE(s.static_info,'{}'::JSONB) AS info,COALESCE(m.healthy AND m.applied_rev=m.target_rev,FALSE) AS healthy FROM servers s LEFT JOIN server_module_status m ON m.server_id=s.id AND m.module='singbox' WHERE s.id=ANY($1) AND s.deleted_at IS NULL")
        .bind(&server_ids).fetch_all(&mut *tx).await?
        .iter()
        .map(|row| Ok((row.try_get("id")?, Host {
            server_id: row.try_get("id")?,
            agent_seen: row.try_get("last_seen")?,
            metrics: row.try_get("metrics")?,
            static_info: row.try_get("info")?,
            runtime_healthy: row.try_get("healthy")?,
        })))
        .collect::<Result<_, sqlx::Error>>()?;
    let probes: Vec<(i64, Value, Value)> = sqlx::query_as("SELECT p.server_id,p.spec,COALESCE((SELECT jsonb_agg(r.result ORDER BY r.sampled_at DESC) FROM (SELECT result,sampled_at FROM probe_results WHERE server_id=p.server_id AND probe_id=p.id AND sampled_at>$2 ORDER BY sampled_at DESC LIMIT 120) r),'[]'::JSONB) FROM network_probes p WHERE p.server_id=ANY($1) ORDER BY p.server_id,p.id")
        .bind(&server_ids).bind(apsp_protocol::telemetry::now_millis() - LATENCY_WINDOW_MS).fetch_all(&mut *tx).await?;
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
        let own: Vec<(Value, Value)> = probes
            .iter()
            .filter(|(id, ..)| id == server)
            .map(|(_, spec, results)| (spec.clone(), results.clone()))
            .collect();
        let latency = probe_latency(&own)
            .or_else(|| statuses.iter().find_map(|status| legacy_latency(status)))
            .unwrap_or_else(|| json!({"monitored":false}));
        host_latency.insert(*server, latency.clone());
        let network = match host.filter(|_| agent) {
            Some(host) => {
                let ips = ip_quality::reported_ips(&host.static_info);
                network(&ip_quality::cached(&state, host.server_id, &ips).await?)
            }
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
        let access = package.is_some_and(|package| plans.contains_key(&package));
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
    fn probe_labels_map_onto_the_three_carriers() {
        assert_eq!(carrier("广东电信"), Some("telecom"));
        assert_eq!(carrier("China Unicom Guangdong"), Some("unicom"));
        assert_eq!(carrier("CMCC"), Some("mobile"));
        assert_eq!(carrier("Cloudflare"), None);
    }

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

    #[test]
    fn carrier_latency_summarizes_the_latest_hour_per_carrier() {
        let rows = vec![
            (
                json!({"carrier":"电信"}),
                json!([{"latency_ms":40.4,"loss_percent":0.0,"sampled_at":2000},{"latency_ms":60.0,"loss_percent":10.0,"sampled_at":1000}]),
            ),
            (
                json!({"carrier":"Example"}),
                json!([{"latency_ms":1.0,"loss_percent":0.0,"sampled_at":3000}]),
            ),
            (
                json!({"carrier":"","monitor":{"network":"mobile"}}),
                json!([{"latency_ms":70.0,"loss_percent":null,"sampled_at":1000}]),
            ),
        ];
        let latency = probe_latency(&rows).unwrap();
        assert_eq!(
            latency["carriers"]["telecom"],
            json!({"latest":40.0,"average":50.0,"loss":5.0})
        );
        assert_eq!(latency["updated_at"], 2);
        assert!(latency["carriers"].get("unicom").is_none());
        assert_eq!(
            latency["carriers"]["mobile"],
            json!({"latest":70.0,"average":70.0,"loss":null})
        );
        assert!(probe_latency(&[]).is_none());
    }
}

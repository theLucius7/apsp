//! Concurrent client address limits for taken-over Nexus nodes (ADR 0099).
//!
//! Devices report the client addresses of the sessions they admitted; the
//! legacy panel's per-user device limits and the addresses its own nodes see
//! arrive through the compatibility bridge. Every active host receives, per
//! limited member, the stat names that share one budget and the addresses the
//! member uses everywhere else.
use crate::{AppState, device_channel};
use apsp_protocol::{
    AdmissionGroup, Envelope, RUNTIME_ADMISSION_CAPABILITY, RUNTIME_ADMISSION_POLICY_KIND,
    RuntimeAdmissionPolicy, RuntimeSessionReport, now_timestamp,
};
use sqlx::{Postgres, Transaction};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
};

const MODULE: &str = "singbox";
/// Addresses older than this no longer describe current use.
pub(crate) const FRESH_SECS: i64 = 60;
const RETAIN_SECS: i64 = 300;

/// Members currently authorized on the hosts' taken-over nodes, as distinct
/// `(server_id, node_id, user_id)` rows for the servers in `$2`. `$1` is the
/// evaluation time, or the users the caller already found allowed (`allowed`).
fn members_sql(allowed: bool) -> String {
    let allowed_users = if allowed {
        "SELECT unnest($1::bigint[]) AS user_id"
    } else {
        "SELECT user_id FROM singbox_entitlements($1) WHERE allowed"
    };
    format!(
        "WITH e AS MATERIALIZED ({allowed_users}) SELECT DISTINCT r.server_id,r.node_id,i.user_id FROM singbox_nexus_runtime_nodes r JOIN singbox_policy_nodes pn ON pn.node_id=r.node_id JOIN singbox_user_policies up ON up.group_id=pn.group_id JOIN e ON e.user_id=up.user_id JOIN singbox_nexus_identities i ON i.user_id=e.user_id WHERE r.server_id=ANY($2)"
    )
}

/// One evaluation of every user's entitlement serves all the hosts: the
/// caller's when it has one, otherwise one at `now`.
async fn members(
    tx: &mut Transaction<'_, Postgres>,
    servers: &[i64],
    now: i64,
    allowed: Option<&[i64]>,
) -> anyhow::Result<Vec<(i64, i64, i64)>> {
    let query = members_sql(allowed.is_some());
    let query = sqlx::query_as(&query);
    let query = match allowed {
        Some(users) => query.bind(users),
        None => query.bind(now),
    };
    Ok(query.bind(servers).fetch_all(&mut **tx).await?)
}

fn principal(value: &str) -> Option<(i64, i64)> {
    let (user, node) = value.strip_prefix('u')?.split_once("_n")?;
    let (user, node) = (user.parse().ok()?, node.parse().ok()?);
    (user > 0 && node > 0 && apsp_compiler::stat_name(user, node) == value).then_some((user, node))
}

/// Replaces the host's admitted addresses with the device's snapshot.
/// Records a device's admitted session addresses for Nexus device limits.
pub async fn ingest(
    state: &AppState,
    server: i64,
    report: RuntimeSessionReport,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        report.valid() && report.module == MODULE,
        "invalid runtime session report"
    );
    let mut tx = state.pool.begin().await?;
    let nodes: Vec<i64> = sqlx::query_scalar("SELECT r.node_id FROM singbox_nexus_runtime_nodes r JOIN singbox_nexus_runtime_hosts h ON h.server_id=r.server_id WHERE r.server_id=$1 AND h.state='active'")
        .bind(server).fetch_all(&mut *tx).await?;
    let nodes: BTreeSet<i64> = nodes.into_iter().collect();
    // Received time, not device time: freshness is judged by the panel clock.
    let now = now_timestamp();
    sqlx::query(
        "DELETE FROM singbox_runtime_session_addresses WHERE server_id=$1 OR observed_at<$2",
    )
    .bind(server)
    .bind(now - RETAIN_SECS)
    .execute(&mut *tx)
    .await?;
    // One statement for the whole snapshot; a repeated address counts once.
    let mut rows = BTreeMap::<(&str, String), (i64, i64)>::new();
    for (name, addresses) in &report.principals {
        let Some((user, node)) = principal(name).filter(|(_, node)| nodes.contains(node)) else {
            continue;
        };
        for address in addresses {
            rows.insert((name.as_str(), address.to_string()), (user, node));
        }
    }
    if !rows.is_empty() {
        let (names, addresses): (Vec<&str>, Vec<String>) = rows.keys().cloned().unzip();
        let (users, nodes): (Vec<i64>, Vec<i64>) = rows.values().copied().unzip();
        sqlx::query("INSERT INTO singbox_runtime_session_addresses(server_id,stat_name,user_id,node_id,address,observed_at) SELECT $1,stat_name,user_id,node_id,address,$6 FROM UNNEST($2::text[],$3::bigint[],$4::bigint[],$5::text[]) AS t(stat_name,user_id,node_id,address) ON CONFLICT(server_id,stat_name,address) DO UPDATE SET observed_at=EXCLUDED.observed_at")
            .bind(server).bind(&names).bind(&users).bind(&nodes).bind(&addresses).bind(now)
            .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

fn addresses(values: impl IntoIterator<Item = String>) -> BTreeSet<IpAddr> {
    values
        .into_iter()
        .filter_map(|value| value.parse().ok())
        .collect()
}

/// A complete replacement policy for each of `servers`, including an empty
/// one to clear old limits. All hosts share one member evaluation and one read
/// of the limits and of the addresses seen on every host.
async fn policies(
    tx: &mut Transaction<'_, Postgres>,
    servers: &[i64],
    now: i64,
    allowed: Option<&[i64]>,
) -> anyhow::Result<BTreeMap<i64, RuntimeAdmissionPolicy>> {
    let mut principals: BTreeMap<i64, BTreeMap<i64, BTreeSet<String>>> = BTreeMap::new();
    for (server, node, user) in members(tx, servers, now, allowed).await? {
        principals
            .entry(server)
            .or_default()
            .entry(user)
            .or_default()
            .insert(apsp_compiler::stat_name(user, node));
    }
    let users: Vec<i64> = principals
        .values()
        .flat_map(|members| members.keys().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let states: BTreeMap<i64, (i32, sqlx::types::Json<Vec<String>>, i64)> =
        sqlx::query_as::<_, (i64, i32, sqlx::types::Json<Vec<String>>, i64)>(
            "SELECT user_id,device_limit,legacy_addresses,observed_at FROM singbox_nexus_device_state WHERE user_id=ANY($1) AND device_limit>0",
        )
        .bind(&users)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .map(|(user, limit, legacy, observed)| (user, (limit, legacy, observed)))
        .collect();
    let limited: Vec<i64> = states.keys().copied().collect();
    // Fresh addresses of limited members on every host; each host's policy
    // takes the ones seen elsewhere.
    let mut seen = BTreeMap::<i64, Vec<(i64, String)>>::new();
    if !limited.is_empty() {
        let rows: Vec<(i64, i64, String)> = sqlx::query_as("SELECT DISTINCT user_id,server_id,address FROM singbox_runtime_session_addresses WHERE user_id=ANY($1) AND observed_at>=$2")
            .bind(&limited).bind(now - FRESH_SECS).fetch_all(&mut **tx).await?;
        for (user, server, address) in rows {
            seen.entry(user).or_default().push((server, address));
        }
    }
    let mut result = BTreeMap::new();
    for &server in servers {
        let mut groups = Vec::new();
        for (user, names) in principals.remove(&server).unwrap_or_default() {
            let Some((limit, legacy, observed)) = states.get(&user) else {
                continue;
            };
            let mut remote = if now - observed <= FRESH_SECS {
                addresses(legacy.0.iter().cloned())
            } else {
                BTreeSet::new()
            };
            remote.extend(addresses(
                seen.get(&user)
                    .into_iter()
                    .flatten()
                    .filter(|(host, _)| *host != server)
                    .map(|(_, address)| address.clone()),
            ));
            groups.push(AdmissionGroup {
                principals: names
                    .into_iter()
                    .take(apsp_protocol::ADMISSION_MAX_PRINCIPALS_PER_GROUP)
                    .collect(),
                limit: (*limit as u32).min(apsp_protocol::ADMISSION_MAX_LIMIT),
                remote: remote
                    .into_iter()
                    .take(apsp_protocol::ADMISSION_MAX_ADDRESSES)
                    .collect(),
            });
        }
        let policy = RuntimeAdmissionPolicy {
            module: MODULE.into(),
            issued_at: now,
            groups,
        };
        anyhow::ensure!(policy.valid(), "computed admission policy is invalid");
        result.insert(server, policy);
    }
    Ok(result)
}

/// Sends the current policy to every active host whose Agent controls sessions.
/// `allowed` is the publication cycle's evaluation of who is allowed now; without
/// it the policies evaluate entitlements themselves.
pub async fn push_policies(state: &AppState, allowed: Option<&[i64]>) -> anyhow::Result<()> {
    // Expire personal data even when every device is offline and no more
    // reports arrive. Keep configured limits, but not stale client addresses.
    let cutoff = now_timestamp() - RETAIN_SECS;
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM singbox_runtime_session_addresses WHERE observed_at<$1")
        .bind(cutoff)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE singbox_nexus_device_state SET legacy_addresses='[]'::jsonb WHERE observed_at<$1 AND legacy_addresses<>'[]'::jsonb")
        .bind(cutoff).execute(&mut *tx).await?;
    tx.commit().await?;
    let hosts: Vec<i64> = sqlx::query_scalar("SELECT h.server_id FROM singbox_nexus_runtime_hosts h JOIN servers s ON s.id=h.server_id WHERE h.state='active' AND s.deleted_at IS NULL AND s.capabilities @> jsonb_build_array($1::text) ORDER BY h.server_id")
        .bind(RUNTIME_ADMISSION_CAPABILITY).fetch_all(&state.pool).await?;
    let mut tx = state.pool.begin().await?;
    let policies = policies(&mut tx, &hosts, now_timestamp(), allowed).await?;
    tx.commit().await?;
    for (server, policy) in policies {
        device_channel::notify(
            state,
            server,
            Envelope::new(RUNTIME_ADMISSION_POLICY_KIND, &policy)?,
        )
        .await;
    }
    Ok(())
}

/// Members with a device limit on this host, and whether the bridge has
/// recently delivered the limits at all.
pub async fn limits_on_host(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    now: i64,
) -> anyhow::Result<(i64, bool)> {
    let users: BTreeSet<i64> = members(tx, &[server], now, None)
        .await?
        .into_iter()
        .map(|(_, _, user)| user)
        .collect();
    let users: Vec<i64> = users.into_iter().collect();
    let limited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM singbox_nexus_device_state WHERE user_id=ANY($1) AND device_limit>0",
    )
    .bind(&users)
    .fetch_one(&mut **tx)
    .await?;
    let synchronized: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_nexus_device_state WHERE observed_at>=$1)",
    )
    .bind(now - 120)
    .fetch_one(&mut **tx)
    .await?;
    Ok((limited, synchronized))
}

/// The device limit policy the publisher would send to this host now, given
/// the publication cycle's allowed users when there are any.
pub async fn admission_policy(
    state: &AppState,
    server_id: i64,
    allowed: Option<&[i64]>,
) -> anyhow::Result<Option<RuntimeAdmissionPolicy>> {
    let mut tx = state.pool.begin().await?;
    let mut policies = policies(&mut tx, &[server_id], now_timestamp(), allowed).await?;
    tx.commit().await?;
    Ok(policies.remove(&server_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_canonical_stat_names_map_to_members_and_nodes() {
        assert_eq!(principal("u43_n2"), Some((43, 2)));
        for invalid in [
            "u043_n2", "u43_n02", "u0_n2", "x43_n2", "u43n2", "u43_n2_x", "u-1_n2",
        ] {
            assert_eq!(principal(invalid), None, "{invalid}");
        }
    }
}

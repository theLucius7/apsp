//! Concurrent client address limits for taken-over Nexus nodes (ADR 0098).
//!
//! Devices report the client addresses of the sessions they admitted; the
//! legacy panel's per-user device limits and the addresses its own nodes see
//! arrive through the compatibility bridge. Every active host receives, per
//! limited member, the stat names that share one budget and the addresses the
//! member uses everywhere else.
use crate::{AppState, agent_api};
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

/// Members currently authorized on a host's taken-over nodes.
pub(crate) const HOST_MEMBERS: &str = "SELECT r.node_id,i.user_id FROM singbox_nexus_runtime_nodes r JOIN singbox_nexus_plan_mappings p ON p.legacy_group_id=ANY(r.legacy_group_ids) JOIN singbox_entitlements($2) e ON e.package_group_id=p.package_group_id AND e.allowed JOIN singbox_nexus_identities i ON i.user_id=e.user_id AND i.import_id=p.import_id WHERE r.server_id=$1";

fn principal(value: &str) -> Option<(i64, i64)> {
    let (user, node) = value.strip_prefix('u')?.split_once("_n")?;
    let (user, node) = (user.parse().ok()?, node.parse().ok()?);
    (user > 0 && node > 0 && apsp_compiler::stat_name(user, node) == value).then_some((user, node))
}

/// Replaces the host's admitted addresses with the device's snapshot.
pub(crate) async fn ingest(
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
    for (name, addresses) in &report.principals {
        let Some((user, node)) = principal(name).filter(|(_, node)| nodes.contains(node)) else {
            continue;
        };
        for address in addresses {
            sqlx::query("INSERT INTO singbox_runtime_session_addresses(server_id,stat_name,user_id,node_id,address,observed_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(server_id,stat_name,address) DO UPDATE SET observed_at=EXCLUDED.observed_at")
                .bind(server).bind(name).bind(user).bind(node).bind(address.to_string()).bind(now)
                .execute(&mut *tx).await?;
        }
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

/// A complete replacement policy, including an empty one to clear old limits.
pub(crate) async fn policy(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    now: i64,
) -> anyhow::Result<Option<RuntimeAdmissionPolicy>> {
    let members: Vec<(i64, i64)> = sqlx::query_as(HOST_MEMBERS)
        .bind(server)
        .bind(now)
        .fetch_all(&mut **tx)
        .await?;
    let mut principals: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    for (node, user) in members {
        principals
            .entry(user)
            .or_default()
            .insert(apsp_compiler::stat_name(user, node));
    }
    let mut groups = Vec::new();
    for (user, names) in principals {
        let state: Option<(i32, sqlx::types::Json<Vec<String>>, i64)> = sqlx::query_as(
            "SELECT device_limit,legacy_addresses,observed_at FROM singbox_nexus_device_state WHERE user_id=$1",
        )
        .bind(user)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((limit, legacy, observed)) = state.filter(|(limit, ..)| *limit > 0) else {
            continue;
        };
        let mut remote = if now - observed <= FRESH_SECS {
            addresses(legacy.0)
        } else {
            BTreeSet::new()
        };
        let elsewhere: Vec<String> = sqlx::query_scalar("SELECT DISTINCT address FROM singbox_runtime_session_addresses WHERE user_id=$1 AND server_id<>$2 AND observed_at>=$3")
            .bind(user).bind(server).bind(now - FRESH_SECS).fetch_all(&mut **tx).await?;
        remote.extend(addresses(elsewhere));
        groups.push(AdmissionGroup {
            principals: names
                .into_iter()
                .take(apsp_protocol::ADMISSION_MAX_PRINCIPALS_PER_GROUP)
                .collect(),
            limit: (limit as u32).min(apsp_protocol::ADMISSION_MAX_LIMIT),
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
    Ok(Some(policy))
}

/// Sends the current policy to every active host whose Agent controls sessions.
pub(crate) async fn push_policies(state: &AppState) -> anyhow::Result<()> {
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
    for server in hosts {
        let mut tx = state.pool.begin().await?;
        let policy = policy(&mut tx, server, now_timestamp()).await?;
        tx.commit().await?;
        if let Some(policy) = policy {
            agent_api::notify(
                state,
                server,
                Envelope::new(RUNTIME_ADMISSION_POLICY_KIND, &policy)?,
            )
            .await;
        }
    }
    Ok(())
}

/// The host's session control secret, fixed when the host was prepared.
pub(crate) async fn secret(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> anyhow::Result<String> {
    Ok(sqlx::query_scalar(
        "SELECT session_secret FROM singbox_nexus_runtime_hosts WHERE server_id=$1",
    )
    .bind(server)
    .fetch_one(&mut **tx)
    .await?)
}

/// Members with a device limit on this host, and whether the bridge has
/// recently delivered the limits at all.
pub(crate) async fn limits_on_host(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    now: i64,
) -> anyhow::Result<(i64, bool)> {
    let limited: i64 = sqlx::query_scalar(&format!("SELECT COUNT(DISTINCT m.user_id) FROM ({HOST_MEMBERS}) m JOIN singbox_nexus_device_state d ON d.user_id=m.user_id WHERE d.device_limit>0"))
        .bind(server).bind(now).fetch_one(&mut **tx).await?;
    let synchronized: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM singbox_nexus_device_state WHERE observed_at>=$1)",
    )
    .bind(now - 120)
    .fetch_one(&mut **tx)
    .await?;
    Ok((limited, synchronized))
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

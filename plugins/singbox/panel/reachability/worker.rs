//! Periodic and requested runs.

use super::{
    alerts,
    budget::{self, Ledger},
    classify::{ReachabilityState, classify},
    globalping::{Client, ReqwestTransport, Transport},
    measure::{self, Outcome},
    store::{self, Claim, Observation, RowMap, TargetRow},
    targets::{self, Target},
};
use crate::{AppState, control_center, error::ApiResult};
use apsp_protocol::now_timestamp;
use futures_util::future::join_all;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{future::Future, net::IpAddr, time::Duration};
use tokio::sync::Semaphore;

const TICK: Duration = Duration::from_secs(15);
/// Targets measured at once; each runs at most two measurements concurrently.
const MEASURE_CONCURRENCY: usize = 4;
const RESOLVE_CONCURRENCY: usize = 8;
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESOLVED_ADDRESSES: usize = 16;
/// Tolerates scheduler jitter so a target checked one interval ago counts as due.
const DUE_SLACK_SECS: i64 = 60;
/// How soon to retry after the provider could not be asked about the budget.
const FAILURE_RETRY_SECS: i64 = 300;
pub(super) const CREDENTIAL_KIND: &str = "external-api";

/// Finds the addresses of a host; replaced in tests so no lookup is made.
pub trait Resolver: Send + Sync {
    fn resolve(&self, host: &str, port: u16) -> impl Future<Output = Vec<IpAddr>> + Send;
}

pub struct SystemResolver;

impl Resolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> Vec<IpAddr> {
        let found = if let Ok(ip) = host.parse::<IpAddr>() {
            vec![ip]
        } else {
            match tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((host, port))).await
            {
                Ok(Ok(addresses)) => addresses
                    .take(MAX_RESOLVED_ADDRESSES)
                    .map(|address| address.ip())
                    .collect(),
                _ => Vec::new(),
            }
        };
        // Private and reserved addresses say nothing and must not leave the panel.
        found
            .into_iter()
            .filter(|ip| apsp_panel_host::ip_quality::public_ip(*ip))
            .collect()
    }
}

/// Mainland probes rarely have IPv6, so an IPv4 address gives the clearer answer.
fn choose_address(addresses: &[IpAddr]) -> Option<IpAddr> {
    addresses
        .iter()
        .find(|ip| ip.is_ipv4())
        .or_else(|| addresses.first())
        .copied()
}

#[derive(Debug, Default, Serialize)]
pub struct RunSummary {
    pub started_at: i64,
    pub finished_at: i64,
    pub manual: bool,
    pub targets: usize,
    pub due: usize,
    pub measured: usize,
    pub deferred: usize,
    pub unresolved: usize,
    pub tests_used: u32,
    pub allowance: Option<u32>,
    pub limit: Option<u32>,
    pub remaining: Option<u32>,
    pub reset_secs: Option<u32>,
    pub error: Option<String>,
}

fn is_due(row: &TargetRow, now: i64, interval_secs: i64) -> bool {
    row.checked_at
        .is_none_or(|checked| checked + interval_secs - DUE_SLACK_SECS <= now)
}

struct Member<'a> {
    target: &'a Target,
    row_id: i64,
}

/// Endpoints resolving to one address and port share one measurement.
struct Group<'a> {
    ip: IpAddr,
    port: u16,
    failing: bool,
    members: Vec<Member<'a>>,
}

impl Group<'_> {
    fn row_ids(&self) -> Vec<i64> {
        self.members.iter().map(|member| member.row_id).collect()
    }
}

struct Context<'a, T> {
    pool: &'a PgPool,
    client: &'a Client<'a, T>,
    ledger: &'a Ledger,
    slots: &'a Semaphore,
    now: i64,
}

async fn resolve_one<R: Resolver>(
    resolver: &R,
    slots: &Semaphore,
    target: &Target,
) -> Option<IpAddr> {
    let _slot = slots.acquire().await.ok()?;
    choose_address(&resolver.resolve(&target.host, target.port).await)
}

async fn process_group<T: Transport>(
    context: &Context<'_, T>,
    group: &Group<'_>,
) -> ApiResult<bool> {
    let _slot = context.slots.acquire().await.map_err(anyhow::Error::from)?;
    // Opting out takes effect at once: an address already queued is not sent.
    let outcome = if store::is_enabled(context.pool).await? {
        measure::measure(context.client, context.ledger, group.ip, group.port).await
    } else {
        Outcome::Deferred
    };
    let Outcome::Measured(evidence) = outcome else {
        store::mark_deferred(context.pool, &group.row_ids(), context.now).await?;
        return Ok(false);
    };
    let observed = classify(&evidence);
    let detail = measure::evidence_detail(&evidence);
    for member in &group.members {
        store::record(
            context.pool,
            Observation {
                target: member.target,
                row_id: member.row_id,
                observed,
                ip: Some(group.ip),
                detail: &detail,
                now: now_timestamp(),
            },
        )
        .await?;
    }
    Ok(true)
}

fn row_for<'a>(rows: &'a RowMap, target: &Target) -> Option<&'a TargetRow> {
    rows.get(&(target.host.clone(), target.port))
}

async fn token_for(pool: &PgPool, claim: &Claim) -> Result<Option<String>, String> {
    let Some(id) = claim.settings.credential_id else {
        return Ok(None);
    };
    let secret = control_center::credentials::resolve_reference_pool(
        pool,
        id,
        CREDENTIAL_KIND,
        "sing-box-reachability",
    )
    .await
    .map_err(|_| "token_unavailable".to_owned())?;
    secret["token"]
        .as_str()
        .map(|token| Some(token.to_owned()))
        .ok_or_else(|| "token_unavailable".to_owned())
}

/// One run. Returns the summary and when the next run should start.
pub(super) async fn run_once<T: Transport, R: Resolver>(
    pool: &PgPool,
    transport: &T,
    resolver: &R,
    claim: &Claim,
) -> ApiResult<(RunSummary, i64)> {
    let now = now_timestamp();
    let interval_secs = i64::from(claim.settings.interval_minutes) * 60;
    let mut summary = RunSummary {
        started_at: now,
        manual: claim.manual,
        ..RunSummary::default()
    };
    let targets = targets::enumerate(pool).await?;
    summary.targets = targets.len();
    let rows = store::sync_targets(pool, &targets, now).await?;

    // Endpoints with no TCP node cost nothing and only need their label set once.
    for target in targets.iter().filter(|target| !target.tcp) {
        let Some(row) = row_for(&rows, target) else {
            continue;
        };
        if row.current_state() != ReachabilityState::NotApplicable {
            store::record(
                pool,
                Observation {
                    target,
                    row_id: row.id,
                    observed: ReachabilityState::NotApplicable,
                    ip: None,
                    detail: &json!({"reason":"udp_only"}),
                    now,
                },
            )
            .await?;
        }
    }

    let mut due: Vec<(&Target, &TargetRow)> = targets
        .iter()
        .filter(|target| target.tcp)
        .filter_map(|target| Some((target, row_for(&rows, target)?)))
        .filter(|(_, row)| claim.manual || is_due(row, now, interval_secs))
        .collect();
    due.sort_by_key(|(_, row)| (row.checked_at.unwrap_or(0), row.id));
    summary.due = due.len();

    let token = match token_for(pool, claim).await {
        Ok(token) => token,
        Err(code) => {
            summary.error = Some(code);
            summary.finished_at = now_timestamp();
            return Ok((summary, now + FAILURE_RETRY_SECS.min(interval_secs)));
        }
    };
    let client = Client::new(transport, token.as_deref());
    #[cfg(test)]
    let client = client.fast_polling();

    let mut deferred_reset = None;
    if !due.is_empty() {
        let limits = match client.limits().await {
            Ok(limits) => limits,
            Err(error) => {
                // Without knowing the budget nothing is sent.
                summary.error = Some(format!("limits_{}", error.code()));
                summary.finished_at = now_timestamp();
                return Ok((summary, now + FAILURE_RETRY_SECS.min(interval_secs)));
            }
        };
        let allowance = budget::allowance(limits);
        summary.allowance = Some(allowance);
        summary.limit = Some(limits.limit);
        summary.remaining = Some(limits.remaining);
        summary.reset_secs = Some(limits.reset_secs);

        let slots = Semaphore::new(RESOLVE_CONCURRENCY);
        let resolved = join_all(
            due.iter()
                .map(|(target, _)| resolve_one(resolver, &slots, target)),
        )
        .await;
        let mut groups: Vec<Group<'_>> = Vec::new();
        for (&(target, row), ip) in due.iter().zip(resolved) {
            let Some(ip) = ip else {
                summary.unresolved += 1;
                store::record(
                    pool,
                    Observation {
                        target,
                        row_id: row.id,
                        observed: ReachabilityState::Unknown,
                        ip: None,
                        detail: &json!({"reason":"no_public_address"}),
                        now: now_timestamp(),
                    },
                )
                .await?;
                continue;
            };
            let member = Member {
                target,
                row_id: row.id,
            };
            match groups
                .iter_mut()
                .find(|group| group.ip == ip && group.port == target.port)
            {
                Some(group) => {
                    group.failing |= row.failing();
                    group.members.push(member);
                }
                None => groups.push(Group {
                    ip,
                    port: target.port,
                    failing: row.failing(),
                    members: vec![member],
                }),
            }
        }

        let failing: Vec<bool> = groups.iter().map(|group| group.failing).collect();
        let admitted = budget::plan(allowance, &failing);
        let ledger = Ledger::new(allowance);
        let slots = Semaphore::new(MEASURE_CONCURRENCY);
        let context = Context {
            pool,
            client: &client,
            ledger: &ledger,
            slots: &slots,
            now,
        };
        let results = join_all(
            groups[..admitted]
                .iter()
                .map(|group| process_group(&context, group)),
        )
        .await;
        for result in results {
            if result? {
                summary.measured += 1;
            } else {
                summary.deferred += 1;
            }
        }
        let later: Vec<i64> = groups[admitted..].iter().flat_map(Group::row_ids).collect();
        store::mark_deferred(pool, &later, now_timestamp()).await?;
        summary.deferred += groups.len() - admitted;
        summary.tests_used = ledger.spent();
        if summary.deferred > 0 {
            deferred_reset = Some(limits.reset_secs);
        }
    }

    store::cleanup(pool, now_timestamp()).await?;
    let finished = now_timestamp();
    summary.finished_at = finished;
    let rows = store::target_rows(pool).await?;
    let earliest_due = targets
        .iter()
        .filter(|target| target.tcp)
        .filter_map(|target| row_for(&rows, target))
        .filter_map(|row| row.checked_at)
        .map(|checked| checked + interval_secs)
        .min();
    let next = budget::next_run_at(finished, interval_secs, earliest_due, deferred_reset);
    Ok((summary, next))
}

/// Claims and performs a run if one is due; never touches notification channels.
pub(super) async fn cycle<T: Transport, R: Resolver>(
    pool: &PgPool,
    transport: &T,
    resolver: &R,
) -> ApiResult<bool> {
    let now = now_timestamp();
    let Some(claim) = store::claim_run(pool, now).await? else {
        return Ok(false);
    };
    let interval_secs = i64::from(claim.settings.interval_minutes) * 60;
    let (summary, next) = match run_once(pool, transport, resolver, &claim).await {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(%error, "reachability run failed");
            let summary = RunSummary {
                started_at: now,
                finished_at: now_timestamp(),
                manual: claim.manual,
                error: Some("run_failed".into()),
                ..RunSummary::default()
            };
            (summary, now + FAILURE_RETRY_SECS.min(interval_secs))
        }
    };
    let summary: Value = serde_json::to_value(&summary).map_err(anyhow::Error::from)?;
    store::finish_run(pool, &summary, next).await?;
    Ok(true)
}

pub(super) async fn run(state: AppState) {
    let transport = match ReqwestTransport::new() {
        Ok(transport) => transport,
        Err(error) => {
            tracing::error!(%error, "reachability monitor cannot start its HTTP client");
            return;
        }
    };
    if let Err(error) = store::release_lease(&state.pool).await {
        tracing::warn!(%error, "reachability lease reset failed");
    }
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        if let Err(error) = alerts::deliver(&state.pool).await {
            tracing::warn!(%error, "reachability alert delivery failed");
        }
        match cycle(&state.pool, &transport, &SystemResolver).await {
            Ok(true) => {
                if let Err(error) = alerts::deliver(&state.pool).await {
                    tracing::warn!(%error, "reachability alert delivery failed");
                }
            }
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "reachability cycle failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_is_preferred_and_ipv6_is_the_fallback() {
        let v4: IpAddr = "192.0.2.1".parse().unwrap();
        let v6: IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(choose_address(&[v6, v4]), Some(v4));
        assert_eq!(choose_address(&[v6]), Some(v6));
        assert_eq!(choose_address(&[]), None);
    }

    #[tokio::test]
    async fn the_system_resolver_keeps_private_addresses_inside_the_panel() {
        for literal in ["10.0.0.1", "127.0.0.1", "192.168.1.1", "::1", "fe80::1"] {
            assert!(SystemResolver.resolve(literal, 443).await.is_empty());
        }
    }
}

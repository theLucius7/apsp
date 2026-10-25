//! Concurrent client address limits for runtime sessions.
//!
//! The panel groups principals (runtime accounting names) that share one
//! budget of distinct client addresses across all devices. Runtime log
//! evidence attributes each session to its principal by client address and
//! port; the control interface lists sessions and closes the ones a group may
//! not open. Decisions follow the previous node software exactly: an address
//! the group already uses here or elsewhere is always admitted; otherwise it is
//! admitted while the group uses fewer addresses than its limit, and once the
//! limit is reached the lowest addresses in lexicographic order win, so every
//! device reaches the same decision. Admitted sessions are never closed later.
#[cfg(target_os = "linux")]
pub(crate) mod journal;

/// Volatile journald namespace runtime units log into: session lines name
/// client and destination addresses and must never reach persistent storage.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) const RUNTIME_LOG_NAMESPACE: &str = "apsp-runtime";

/// Per-session runtime log levels, never shown to administrators.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn session_level(text: &str) -> bool {
    // Level labels may carry terminal colors (ESC '[' ... letter).
    let mut plain = String::with_capacity(text.len().min(256));
    let mut characters = text.chars().take(256);
    while let Some(character) = characters.next() {
        if character == '\x1b' {
            for next in characters.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }
    plain.split_whitespace().take(4).any(|token| {
        ["INFO", "DEBUG", "TRACE"]
            .iter()
            .any(|level| token.starts_with(level))
    })
}

use apsp_adapter_sdk::{ActiveSession, SessionEvidence};
use apsp_protocol::{
    ADMISSION_REMOTE_FRESHNESS_SECS, RuntimeAdmissionPolicy, RuntimeSessionReport,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    net::{IpAddr, SocketAddr},
    time::{Duration, Instant},
};

const EVIDENCE_LIMIT: usize = 65_536;
const PENDING_TTL: Duration = Duration::from_secs(60);
const ATTRIBUTION_TTL: Duration = Duration::from_secs(300);
/// A session whose log evidence has not arrived yet waits this long before it
/// is admitted without attribution; late evidence still makes it count.
const ATTRIBUTION_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Pending {
    source: Option<SocketAddr>,
    principal: Option<String>,
    at: Instant,
}

#[derive(Debug, Clone)]
struct Decision {
    principal: Option<String>,
    address: IpAddr,
    admitted: bool,
}

#[derive(Debug)]
struct Group {
    limit: usize,
    remote: BTreeSet<IpAddr>,
}

#[derive(Debug)]
struct Policy {
    group_of: HashMap<String, usize>,
    groups: Vec<Group>,
    received: Instant,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Tick {
    /// Sessions to close: rejected and still open.
    pub close: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Engine {
    pending: HashMap<String, Pending>,
    attributions: HashMap<SocketAddr, (String, Instant)>,
    decided: HashMap<String, Decision>,
    unattributed: HashMap<String, Instant>,
    policy: Option<Policy>,
    observed: bool,
}

impl Engine {
    pub(crate) fn set_policy(&mut self, policy: &RuntimeAdmissionPolicy, now: Instant) -> bool {
        if !policy.valid() {
            return false;
        }
        let mut group_of = HashMap::new();
        let groups = policy
            .groups
            .iter()
            .enumerate()
            .map(|(index, group)| {
                for principal in &group.principals {
                    group_of.insert(principal.clone(), index);
                }
                Group {
                    limit: group.limit as usize,
                    remote: group.remote.iter().copied().collect(),
                }
            })
            .collect();
        self.policy = Some(Policy {
            group_of,
            groups,
            received: now,
        });
        true
    }

    /// Forget everything when session control is disabled or the runtime changes.
    pub(crate) fn reset(&mut self) {
        let policy = self.policy.take();
        *self = Self::default();
        self.policy = policy;
    }

    pub(crate) fn evidence(&mut self, evidence: SessionEvidence, now: Instant) {
        if self.pending.len() >= EVIDENCE_LIMIT && !self.pending.contains_key(&evidence.log_id) {
            self.pending
                .retain(|_, pending| now.duration_since(pending.at) < PENDING_TTL);
            if self.pending.len() >= EVIDENCE_LIMIT {
                return;
            }
        }
        let entry = self.pending.entry(evidence.log_id).or_insert(Pending {
            source: None,
            principal: None,
            at: now,
        });
        entry.source = entry.source.or(evidence.source);
        if entry.principal.is_none() {
            entry.principal = evidence.principal;
        }
        if let (Some(source), Some(principal)) = (entry.source, entry.principal.clone())
            && (self.attributions.len() < EVIDENCE_LIMIT || self.attributions.contains_key(&source))
        {
            self.attributions.insert(source, (principal, now));
        }
    }

    fn remote(&self, group: &Group, now: Instant) -> BTreeSet<IpAddr> {
        let fresh = self.policy.as_ref().is_some_and(|policy| {
            now.duration_since(policy.received)
                <= Duration::from_secs(ADMISSION_REMOTE_FRESHNESS_SECS)
        });
        if fresh {
            group.remote.clone()
        } else {
            BTreeSet::new()
        }
    }

    fn local(&self, group: usize) -> BTreeSet<IpAddr> {
        let Some(policy) = &self.policy else {
            return BTreeSet::new();
        };
        self.decided
            .values()
            .filter(|decision| {
                decision.admitted
                    && decision
                        .principal
                        .as_ref()
                        .and_then(|principal| policy.group_of.get(principal))
                        == Some(&group)
            })
            .map(|decision| decision.address)
            .collect()
    }

    fn admit(&self, principal: Option<&String>, address: IpAddr, now: Instant) -> bool {
        let (Some(policy), Some(principal)) = (&self.policy, principal) else {
            return true;
        };
        let Some(&index) = policy.group_of.get(principal) else {
            return true;
        };
        let group = &policy.groups[index];
        let local = self.local(index);
        let remote = self.remote(group, now);
        if local.contains(&address) || remote.contains(&address) {
            return true;
        }
        let mut all: BTreeSet<IpAddr> = local.union(&remote).copied().collect();
        if all.len() < group.limit {
            return true;
        }
        // Over the limit: the lowest addresses in string order win, the same
        // ordering the previous node software and every other device use.
        all.insert(address);
        let mut ordered: Vec<String> = all.iter().map(ToString::to_string).collect();
        ordered.sort();
        let candidate = address.to_string();
        ordered
            .iter()
            .take(group.limit)
            .any(|value| *value == candidate)
    }

    pub(crate) fn tick(&mut self, active: &[ActiveSession], now: Instant) -> Tick {
        self.pending
            .retain(|_, pending| now.duration_since(pending.at) < PENDING_TTL);
        let live: HashMap<&str, SocketAddr> = active
            .iter()
            .map(|session| (session.id.as_str(), session.source))
            .collect();
        let sources: BTreeSet<SocketAddr> = live.values().copied().collect();
        self.attributions.retain(|source, (_, at)| {
            sources.contains(source) || now.duration_since(*at) < ATTRIBUTION_TTL
        });
        self.decided.retain(|id, _| live.contains_key(id.as_str()));
        self.unattributed
            .retain(|id, _| live.contains_key(id.as_str()));
        // Evidence that arrives after a session was admitted still makes it
        // count against its group; it is never closed afterwards.
        for (id, decision) in &mut self.decided {
            if decision.principal.is_none()
                && let Some((principal, _)) = live
                    .get(id.as_str())
                    .and_then(|source| self.attributions.get(source))
            {
                decision.principal = Some(principal.clone());
            }
        }
        // The first observation after a start or reset admits every existing
        // session; enforcement applies to sessions opened afterwards.
        let startup = !self.observed;
        self.observed = true;
        let mut undecided: Vec<(Instant, &ActiveSession)> = active
            .iter()
            .filter(|session| !self.decided.contains_key(&session.id))
            .map(|session| {
                let at = self
                    .attributions
                    .get(&session.source)
                    .map(|(_, at)| *at)
                    .unwrap_or(now);
                (at, session)
            })
            .collect();
        // Approximate arrival order: evidence time, then identifier.
        undecided.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.id.cmp(&right.1.id)));
        let mut close = Vec::new();
        for (_, session) in undecided {
            let principal = self
                .attributions
                .get(&session.source)
                .map(|(principal, _)| principal.clone());
            if principal.is_none() && !startup {
                let first = *self.unattributed.entry(session.id.clone()).or_insert(now);
                if now.duration_since(first) < ATTRIBUTION_GRACE {
                    continue;
                }
            }
            let admitted = startup || self.admit(principal.as_ref(), session.source.ip(), now);
            self.decided.insert(
                session.id.clone(),
                Decision {
                    principal,
                    address: session.source.ip(),
                    admitted,
                },
            );
        }
        for (id, decision) in &self.decided {
            if !decision.admitted {
                close.push(id.clone());
            }
        }
        close.sort();
        Tick { close }
    }

    /// Distinct client addresses of admitted, attributed sessions.
    pub(crate) fn report(&self, module: &str, observed_at: i64) -> RuntimeSessionReport {
        let mut principals: BTreeMap<String, BTreeSet<IpAddr>> = BTreeMap::new();
        for decision in self.decided.values() {
            if let (true, Some(principal)) = (decision.admitted, &decision.principal) {
                let addresses = principals.entry(principal.clone()).or_default();
                if addresses.len() < apsp_protocol::ADMISSION_MAX_ADDRESSES {
                    addresses.insert(decision.address);
                }
            }
        }
        RuntimeSessionReport {
            module: module.into(),
            observed_at,
            principals: principals
                .into_iter()
                .take(apsp_protocol::SESSION_REPORT_MAX_PRINCIPALS)
                .map(|(principal, addresses)| (principal, addresses.into_iter().collect()))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apsp_protocol::AdmissionGroup;

    fn session(id: &str, source: &str) -> ActiveSession {
        ActiveSession {
            id: id.into(),
            source: source.parse().unwrap(),
        }
    }

    fn attribute(engine: &mut Engine, log: &str, source: &str, principal: &str, now: Instant) {
        engine.evidence(
            SessionEvidence {
                log_id: log.into(),
                source: Some(source.parse().unwrap()),
                principal: None,
            },
            now,
        );
        engine.evidence(
            SessionEvidence {
                log_id: log.into(),
                source: None,
                principal: Some(principal.into()),
            },
            now,
        );
    }

    fn limited(limit: u32, remote: &[&str]) -> RuntimeAdmissionPolicy {
        RuntimeAdmissionPolicy {
            module: "proxy".into(),
            issued_at: 1,
            groups: vec![AdmissionGroup {
                principals: vec!["u1_n1".into(), "u1_n2".into()],
                limit,
                remote: remote.iter().map(|value| value.parse().unwrap()).collect(),
            }],
        }
    }

    /// An engine past its startup observation, so enforcement applies.
    fn running(policy: &RuntimeAdmissionPolicy, now: Instant) -> Engine {
        let mut engine = Engine::default();
        assert!(engine.set_policy(policy, now));
        assert_eq!(engine.tick(&[], now), Tick::default());
        engine
    }

    #[test]
    fn administrators_see_warnings_but_never_session_lines() {
        assert!(session_level(
            "+0000 2026-10-25 01:00:00 INFO [1 0ms] inbound/vless[in]: inbound connection from 192.0.2.1:1"
        ));
        assert!(session_level(
            "+0000 2026-10-25 01:00:00 \x1b[36mINFO\x1b[0m [\x1b[38;5;9m1\x1b[0m 0ms] x"
        ));
        assert!(session_level("DEBUG[0000] dns: lookup"));
        assert!(!session_level(
            "+0000 2026-10-25 01:00:00 \x1b[33mWARN\x1b[0m inbound/vless[in]: failed"
        ));
        assert!(!session_level(
            "+0000 2026-10-25 01:00:00 ERROR router: rule set missing"
        ));
    }

    #[test]
    fn known_addresses_pass_new_ones_fill_the_limit_and_extra_devices_are_closed() {
        let now = Instant::now();
        let mut engine = running(&limited(2, &[]), now);
        let mut active = Vec::new();
        for (id, source) in [
            ("a", "192.0.2.1:1"),
            ("b", "192.0.2.2:1"),
            ("c", "192.0.2.1:2"),
        ] {
            attribute(&mut engine, id, source, "u1_n1", now);
            active.push(session(id, source));
        }
        assert!(engine.tick(&active, now).close.is_empty());
        // A third address sorts after both admitted ones and is refused.
        attribute(&mut engine, "d", "192.0.2.9:1", "u1_n2", now);
        active.push(session("d", "192.0.2.9:1"));
        assert_eq!(engine.tick(&active, now).close, vec!["d".to_string()]);
        // It stays refused until the runtime has closed it.
        assert_eq!(engine.tick(&active, now).close, vec!["d".to_string()]);
        active.pop();
        assert!(engine.tick(&active, now).close.is_empty());
        let report = engine.report("proxy", 9);
        assert_eq!(report.principals["u1_n1"].len(), 2);
        assert!(!report.principals.contains_key("u1_n2"));
    }

    #[test]
    fn addresses_in_use_elsewhere_count_and_the_lowest_address_wins_when_full() {
        let now = Instant::now();
        let mut engine = running(&limited(2, &["192.0.2.5"]), now);
        attribute(&mut engine, "a", "192.0.2.8:1", "u1_n1", now);
        let mut active = vec![session("a", "192.0.2.8:1")];
        assert!(
            engine.tick(&active, now).close.is_empty(),
            "second address overall"
        );
        // The group already uses two addresses; a lower third one still wins.
        attribute(&mut engine, "b", "192.0.2.1:1", "u1_n1", now);
        active.push(session("b", "192.0.2.1:1"));
        assert!(engine.tick(&active, now).close.is_empty());
        // An address the group uses on another device is always admitted.
        attribute(&mut engine, "c", "192.0.2.5:7", "u1_n2", now);
        active.push(session("c", "192.0.2.5:7"));
        assert!(engine.tick(&active, now).close.is_empty());
        attribute(&mut engine, "d", "192.0.2.9:1", "u1_n1", now);
        active.push(session("d", "192.0.2.9:1"));
        assert_eq!(engine.tick(&active, now).close, vec!["d".to_string()]);
    }

    #[test]
    fn stale_remote_state_falls_back_to_local_addresses_but_keeps_limits() {
        let start = Instant::now();
        let mut engine = running(&limited(1, &["192.0.2.1"]), start);
        let later = start + Duration::from_secs(ADMISSION_REMOTE_FRESHNESS_SECS + 1);
        attribute(&mut engine, "a", "192.0.2.9:1", "u1_n1", later);
        let mut active = vec![session("a", "192.0.2.9:1")];
        assert!(
            engine.tick(&active, later).close.is_empty(),
            "remote use expired"
        );
        // String order, as on every other device: "192.0.2.99" sorts last.
        attribute(&mut engine, "b", "192.0.2.99:1", "u1_n1", later);
        active.push(session("b", "192.0.2.99:1"));
        assert_eq!(engine.tick(&active, later).close, vec!["b".to_string()]);
    }

    #[test]
    fn empty_replacement_policy_removes_the_last_device_limit() {
        let now = Instant::now();
        let mut engine = running(&limited(1, &["192.0.2.1"]), now);
        attribute(&mut engine, "a", "192.0.2.9:1", "u1_n1", now);
        assert_eq!(
            engine.tick(&[session("a", "192.0.2.9:1")], now).close,
            vec!["a"]
        );
        engine.tick(&[], now);
        let mut cleared = limited(1, &[]);
        cleared.groups.clear();
        assert!(engine.set_policy(&cleared, now));
        attribute(&mut engine, "b", "192.0.2.9:2", "u1_n1", now);
        assert!(
            engine
                .tick(&[session("b", "192.0.2.9:2")], now)
                .close
                .is_empty()
        );
    }

    #[test]
    fn startup_admits_existing_sessions_and_unattributed_ones_wait_then_pass_uncounted() {
        let now = Instant::now();
        let mut engine = Engine::default();
        assert!(engine.set_policy(&limited(1, &[]), now));
        for (id, source) in [("a", "192.0.2.1:1"), ("b", "192.0.2.2:1")] {
            attribute(&mut engine, id, source, "u1_n1", now);
        }
        let mut active = vec![session("a", "192.0.2.1:1"), session("b", "192.0.2.2:1")];
        assert!(
            engine.tick(&active, now).close.is_empty(),
            "never close on start"
        );
        active.push(session("c", "192.0.2.3:1"));
        assert!(engine.tick(&active, now).close.is_empty());
        assert!(!engine.decided.contains_key("c"), "waits for its evidence");
        let late = now + ATTRIBUTION_GRACE;
        assert!(engine.tick(&active, late).close.is_empty());
        assert_eq!(engine.decided["c"].principal, None);
        assert_eq!(engine.report("proxy", 1).principals["u1_n1"].len(), 2);
        // Late evidence attributes the admitted session, so it now counts.
        attribute(&mut engine, "c", "192.0.2.3:1", "u1_n1", late);
        assert!(
            engine.tick(&active, late).close.is_empty(),
            "never closed later"
        );
        assert_eq!(engine.report("proxy", 1).principals["u1_n1"].len(), 3);
        attribute(&mut engine, "d", "192.0.2.4:1", "u1_n1", late);
        active.push(session("d", "192.0.2.4:1"));
        assert_eq!(engine.tick(&active, late).close, vec!["d".to_string()]);
    }

    #[test]
    fn principals_without_a_budget_and_evidence_across_batches_are_handled() {
        let now = Instant::now();
        let mut engine = running(&limited(1, &[]), now);
        engine.evidence(
            SessionEvidence {
                log_id: "x".into(),
                source: Some("192.0.2.1:1".parse().unwrap()),
                principal: None,
            },
            now,
        );
        let active = vec![session("s", "192.0.2.1:1")];
        assert!(engine.tick(&active, now).close.is_empty());
        engine.evidence(
            SessionEvidence {
                log_id: "x".into(),
                source: None,
                principal: Some("u9_n1".into()),
            },
            now,
        );
        assert!(engine.tick(&active, now).close.is_empty());
        assert_eq!(engine.decided["s"].principal.as_deref(), Some("u9_n1"));
        assert!(
            !engine.set_policy(&limited(0, &[]), now),
            "invalid policy refused"
        );
    }
}

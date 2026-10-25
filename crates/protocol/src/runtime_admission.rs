//! Concurrent client address limits for runtime sessions.
//!
//! A principal is the opaque accounting name the runtime logs for an
//! authenticated session. The panel groups principals that share one budget
//! and states how many distinct client addresses the group may use at once,
//! together with the addresses it is currently using on other devices. The
//! Agent reports the addresses of the sessions it admitted.
use crate::runtime_control::runtime_module_valid;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
};

pub const RUNTIME_ADMISSION_CAPABILITY: &str = "runtime:session-admission-v1";
pub const RUNTIME_ADMISSION_POLICY_KIND: &str = "runtime.admission.policy";
pub const RUNTIME_SESSION_REPORT_KIND: &str = "runtime.sessions.report";
pub const ADMISSION_MAX_GROUPS: usize = 4096;
pub const ADMISSION_MAX_PRINCIPALS_PER_GROUP: usize = 256;
pub const ADMISSION_MAX_ADDRESSES: usize = 64;
pub const ADMISSION_MAX_LIMIT: u32 = 1024;
pub const SESSION_REPORT_MAX_PRINCIPALS: usize = 8192;
/// Remote addresses older than this no longer describe current use; the
/// Agent then checks its own sessions only.
pub const ADMISSION_REMOTE_FRESHNESS_SECS: u64 = 60;

/// Accounting names are compiler-generated; reject anything else outright.
pub fn principal_valid(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionGroup {
    pub principals: Vec<String>,
    /// Maximum distinct client addresses across all devices.
    pub limit: u32,
    /// Addresses this group currently uses on other devices.
    pub remote: Vec<IpAddr>,
}

/// Panel to Agent. A newer policy replaces the previous one entirely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeAdmissionPolicy {
    pub module: String,
    pub issued_at: i64,
    pub groups: Vec<AdmissionGroup>,
}

impl RuntimeAdmissionPolicy {
    pub fn valid(&self) -> bool {
        if !runtime_module_valid(&self.module)
            || self.issued_at <= 0
            || self.groups.len() > ADMISSION_MAX_GROUPS
        {
            return false;
        }
        let mut seen = BTreeSet::new();
        self.groups.iter().all(|group| {
            (1..=ADMISSION_MAX_LIMIT).contains(&group.limit)
                && (1..=ADMISSION_MAX_PRINCIPALS_PER_GROUP).contains(&group.principals.len())
                && group.remote.len() <= ADMISSION_MAX_ADDRESSES
                && group.remote.iter().collect::<BTreeSet<_>>().len() == group.remote.len()
                && group
                    .principals
                    .iter()
                    .all(|principal| principal_valid(principal) && seen.insert(principal.clone()))
        })
    }
}

/// Agent to panel: a complete snapshot of admitted sessions' client addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSessionReport {
    pub module: String,
    pub observed_at: i64,
    pub principals: BTreeMap<String, Vec<IpAddr>>,
}

impl RuntimeSessionReport {
    pub fn valid(&self) -> bool {
        runtime_module_valid(&self.module)
            && self.observed_at > 0
            && self.principals.len() <= SESSION_REPORT_MAX_PRINCIPALS
            && self.principals.iter().all(|(principal, addresses)| {
                principal_valid(principal)
                    && (1..=ADMISSION_MAX_ADDRESSES).contains(&addresses.len())
                    && addresses.iter().collect::<BTreeSet<_>>().len() == addresses.len()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> RuntimeAdmissionPolicy {
        RuntimeAdmissionPolicy {
            module: "singbox".into(),
            issued_at: 100,
            groups: vec![AdmissionGroup {
                principals: vec!["u1_n2".into(), "u1_n3".into()],
                limit: 3,
                remote: vec!["192.0.2.1".parse().unwrap()],
            }],
        }
    }

    #[test]
    fn policies_bound_groups_limits_and_principals() {
        assert!(policy().valid());
        let mut duplicate = policy();
        duplicate.groups.push(duplicate.groups[0].clone());
        assert!(!duplicate.valid(), "a principal belongs to one budget");
        let mut zero = policy();
        zero.groups[0].limit = 0;
        assert!(!zero.valid());
        let mut name = policy();
        name.groups[0].principals[0] = "u1 n2".into();
        assert!(!name.valid());
        let mut repeated = policy();
        repeated.groups[0].remote.push("192.0.2.1".parse().unwrap());
        assert!(!repeated.valid());
        let mut module = policy();
        module.module = "../x".into();
        assert!(!module.valid());
    }

    #[test]
    fn reports_carry_distinct_addresses_and_reject_unknown_fields() {
        let report = RuntimeSessionReport {
            module: "singbox".into(),
            observed_at: 5,
            principals: BTreeMap::from([(
                "u1_n2".into(),
                vec!["192.0.2.1".parse().unwrap(), "2001:db8::1".parse().unwrap()],
            )]),
        };
        assert!(report.valid());
        let mut empty = report.clone();
        empty.principals.insert("u2_n2".into(), Vec::new());
        assert!(!empty.valid());
        let value =
            serde_json::json!({"module":"singbox","observed_at":5,"principals":{},"extra":1});
        assert!(serde_json::from_value::<RuntimeSessionReport>(value).is_err());
    }
}

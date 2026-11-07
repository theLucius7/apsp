//! Per-host budgets for authenticated proxy principals; never client settings.
use crate::CompileError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UserLimits {
    pub upload_mbps: u32,
    pub download_mbps: u32,
    pub max_connections: u32,
}

impl UserLimits {
    pub fn validate(&self) -> Result<(), CompileError> {
        if self.upload_mbps > 1_000_000
            || self.download_mbps > 1_000_000
            || self.max_connections > 1_000_000
        {
            return Err(CompileError::InvalidUserLimits);
        }
        Ok(())
    }
}

/// Add one budget per user to a fully compiled host. Accounting names already
/// include only authenticated public ingress, so internal chain hops are not
/// charged twice. Neither subscription rendering nor credentials are changed.
pub fn apply_user_limits(
    config: &str,
    limits: &BTreeMap<i64, UserLimits>,
) -> Result<String, CompileError> {
    for (id, limit) in limits {
        if *id <= 0 {
            return Err(CompileError::InvalidUserId(*id));
        }
        limit.validate()?;
    }
    let mut value: Value = serde_json::from_str(config)?;
    let names = value
        .pointer("/experimental/v2ray_api/stats/users")
        .and_then(Value::as_array)
        .ok_or(CompileError::InvalidUserLimits)?;
    let mut groups: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    for name in names {
        let name = name.as_str().ok_or(CompileError::InvalidUserLimits)?;
        let (user, node) = name
            .strip_prefix('u')
            .and_then(|s| s.split_once("_n"))
            .ok_or(CompileError::InvalidUserLimits)?;
        let user: i64 = user.parse().map_err(|_| CompileError::InvalidUserLimits)?;
        let node: i64 = node.parse().map_err(|_| CompileError::InvalidUserLimits)?;
        if user <= 0 || node <= 0 || crate::stat_name(user, node) != name {
            return Err(CompileError::InvalidUserLimits);
        }
        if limits
            .get(&user)
            .is_some_and(|limit| *limit != UserLimits::default())
        {
            groups.entry(user).or_default().insert(name.to_owned());
        }
    }
    let entries: Vec<Value> = groups.into_iter().map(|(id, principals)| {
        let limit = limits[&id];
        json!({"principals":principals,"upload_bytes_per_second":u64::from(limit.upload_mbps)*125_000,
            "download_bytes_per_second":u64::from(limit.download_mbps)*125_000,"max_connections":limit.max_connections})
    }).collect();
    if entries.is_empty() {
        return Ok(config.to_owned());
    }
    if value.get("route").is_none() {
        value["route"] = json!({});
    }
    let route = value["route"]
        .as_object_mut()
        .ok_or(CompileError::InvalidUserLimits)?;
    route.insert("apsp_user_limits".into(), json!(entries));
    Ok(serde_json::to_string_pretty(&value)? + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budgets_share_nodes_but_exclude_absent_and_unlimited_users() {
        let config = json!({"experimental":{"v2ray_api":{"stats":{"users":["u2_n3","u1_n9","u1_n4","u4_n3"]}}}}).to_string();
        let limits = BTreeMap::from([
            (
                1,
                UserLimits {
                    upload_mbps: 8,
                    download_mbps: 16,
                    max_connections: 3,
                },
            ),
            (2, UserLimits::default()),
            (
                3,
                UserLimits {
                    max_connections: 1,
                    ..Default::default()
                },
            ),
        ]);
        let output = apply_user_limits(&config, &limits).unwrap();
        let value: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            value["route"]["apsp_user_limits"],
            json!([{"principals":["u1_n4","u1_n9"],"upload_bytes_per_second":1000000,"download_bytes_per_second":2000000,"max_connections":3}])
        );
        assert_eq!(output, apply_user_limits(&config, &limits).unwrap());
        assert_eq!(
            config,
            apply_user_limits(&config, &BTreeMap::new()).unwrap()
        );
    }
    #[test]
    fn rejects_invalid_identity_and_limit_overflow() {
        assert!(
            UserLimits {
                upload_mbps: 1_000_001,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        let config =
            json!({"experimental":{"v2ray_api":{"stats":{"users":["u01_n3"]}}}}).to_string();
        assert!(apply_user_limits(&config, &BTreeMap::new()).is_err());
    }
}

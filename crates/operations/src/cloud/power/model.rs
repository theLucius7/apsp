//! The stored shapes of a resource's power automation in `cloud_resource_automation`
//! (ADR 0137 step 7): the power policy, the power state last read from the cloud and the CDT
//! guard's reading. What they allow is decided in `policy`, `state` and `guard`.

use serde::{Deserialize, Serialize};

/// The policy a resource has until one is saved, as stored: the column default of
/// `cloud_resource_automation.power_policy` (it lacks the fields added since, which read as
/// their defaults). The operations page shows stored policies as they are.
pub const DEFAULT_POLICY: &str = r#"{"enabled": false, "limit_gb": 100, "keepalive": false, "stop_mode": "KeepCharging", "stop_time": "23:00", "start_time": "08:00", "schedule_enabled": false, "threshold_action": "off", "threshold_percent": 95, "utc_offset_minutes": 480}"#;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub enabled: bool,
    pub stop_mode: String,
    pub threshold_action: String,
    pub limit_gb: u64,
    pub threshold_percent: u64,
    pub schedule_enabled: bool,
    #[serde(default)]
    pub schedule_mode: ScheduleMode,
    #[serde(default)]
    pub start_cron: String,
    #[serde(default)]
    pub stop_cron: String,
    pub start_time: String,
    pub stop_time: String,
    pub utc_offset_minutes: i64,
    pub keepalive: bool,
    #[serde(default)]
    pub cdt_guard: Option<GuardConfig>,
}

impl Default for Policy {
    /// [`DEFAULT_POLICY`] as read.
    fn default() -> Self {
        Self {
            enabled: false,
            stop_mode: "KeepCharging".into(),
            threshold_action: "off".into(),
            limit_gb: 100,
            threshold_percent: 95,
            schedule_enabled: false,
            schedule_mode: ScheduleMode::Daily,
            start_cron: String::new(),
            stop_cron: String::new(),
            start_time: "08:00".into(),
            stop_time: "23:00".into(),
            utc_offset_minutes: 480,
            keepalive: false,
            cdt_guard: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleMode {
    #[default]
    Daily,
    Cron,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub cloud_id: String,
    pub region: String,
    pub status: String,
    pub stopped_mode: Option<String>,
    pub charge_type: String,
    pub network_type: String,
    pub spot_strategy: String,
    pub interruption_behavior: Option<String>,
    pub public_ips: Vec<String>,
    #[serde(default)]
    pub public_ipv6: Vec<String>,
    pub locked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardConfig {
    pub threshold_bytes: u64,
    pub jump_bytes: u64,
    pub auto_start: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GuardState {
    pub billing_cycle: String,
    pub region: String,
    pub cloud_id: String,
    pub key_digest: String,
    pub account_revision: i64,
    pub max_usage_bytes: u64,
    pub max_jump_bytes: u64,
    pub queried_at: i64,
    #[serde(default)]
    pub cdt_observed: bool,
}

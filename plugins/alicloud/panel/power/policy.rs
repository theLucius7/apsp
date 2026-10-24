use super::super::{
    billing,
    model::{Account, Resource},
};
use crate::error::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct Policy {
    pub enabled: bool,
    pub stop_mode: String,
    pub threshold_action: String,
    pub limit_gb: u64,
    pub threshold_percent: u64,
    pub schedule_enabled: bool,
    pub start_time: String,
    pub stop_time: String,
    pub utc_offset_minutes: i64,
    pub keepalive: bool,
    #[serde(default)]
    pub cdt_guard: Option<super::guard::Config>,
}
impl Policy {
    pub fn validate(&self) -> ApiResult<()> {
        if let Some(guard) = &self.cdt_guard {
            guard.validate()?;
            if self.stop_mode != "StopCharging"
                || self.threshold_action != "off"
                || self.schedule_enabled
                || self.keepalive
            {
                return Err(ApiError::BadRequest(
                    "CDT 脚本守护使用节省停机，不能同时启用账单阈值、每日计划或抢占式保活".into(),
                ));
            }
        }
        if !valid_mode(&self.stop_mode)
            || !matches!(self.threshold_action.as_str(), "off" | "notify" | "stop")
            || !(1..=1_000_000_000).contains(&self.limit_gb)
            || !(1..=100).contains(&self.threshold_percent)
            || !(-720..=840).contains(&self.utc_offset_minutes)
            || self.utc_offset_minutes % 15 != 0
            || minute(&self.start_time).is_none()
            || minute(&self.stop_time).is_none()
            || self.start_time == self.stop_time
        {
            return Err(ApiError::BadRequest(
                "请核对停机模式、流量额度、1–100% 阈值及不同的每日开关机时间（00:00–23:59）".into(),
            ));
        }
        Ok(())
    }
    pub fn exceeded(&self, account: &Account, now: i64) -> Option<bool> {
        if !account.enabled || account.error_code.is_some() {
            return None;
        }
        let bill = account.bill.as_ref()?;
        if bill.month != billing::month(now) || bill.queried_at > now || now - bill.queried_at > 900
        {
            return None;
        }
        Some(
            bill.usage_micro_gb? as u128 * 100
                >= self.limit_gb as u128 * 1_000_000 * self.threshold_percent as u128,
        )
    }
    pub fn blocks_start(&self, account: &Account, resource: &Resource, now: i64) -> bool {
        if self.enabled && self.cdt_guard.is_some() {
            return super::guard::exceeded(account, resource, now) != Some(false);
        }
        self.enabled
            && self.threshold_action == "stop"
            && (resource.threshold_hold || self.exceeded(account, now) != Some(false))
    }
    pub fn in_window(&self, now: i64) -> bool {
        let minute_now = (now + self.utc_offset_minutes * 60).rem_euclid(86400) / 60;
        let start = minute(&self.start_time).unwrap_or(0);
        let stop = minute(&self.stop_time).unwrap_or(0);
        if start < stop {
            minute_now >= start && minute_now < stop
        } else {
            minute_now >= start || minute_now < stop
        }
    }
    pub fn occurrence(&self, action: &str, now: i64) -> Option<i64> {
        let clock = if action == "start" {
            &self.start_time
        } else {
            &self.stop_time
        };
        let offset = self.utc_offset_minutes * 60;
        let local = now + offset;
        let mut event = local.div_euclid(86400) * 86400 + minute(clock)? * 60 - offset;
        if event > now {
            event -= 86400;
        }
        (now - event < 600).then_some(event)
    }
}
pub(super) fn valid_mode(mode: &str) -> bool {
    matches!(mode, "KeepCharging" | "StopCharging")
}
fn minute(value: &str) -> Option<i64> {
    if value.len() != 5 || value.as_bytes()[2] != b':' {
        return None;
    }
    let (hour, minute) = value.split_once(':')?;
    if !hour
        .bytes()
        .chain(minute.bytes())
        .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let hour: i64 = hour.parse().ok()?;
    let minute: i64 = minute.parse().ok()?;
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

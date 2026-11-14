use super::model::{Policy, ScheduleMode};
use crate::cloud::automation::Automated;
use crate::error::{ApiError, ApiResult};
use apsp_costs::cloud::accounts::AccountBilling;
use apsp_resources::cloud::model::Account;

/// What a resource's power policy allows.
impl Policy {
    pub(crate) fn validate(&self) -> ApiResult<()> {
        if let Some(guard) = &self.cdt_guard {
            guard.validate()?;
            if self.stop_mode != "StopCharging" || self.threshold_action != "off" {
                return Err(ApiError::BadRequest(
                    "CDT 守护使用节省停机，不能同时启用账单阈值".into(),
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
            || (self.schedule_mode == ScheduleMode::Daily && self.start_time == self.stop_time)
        {
            return Err(ApiError::BadRequest(
                "请核对停机模式、流量额度、1–100% 阈值及不同的每日开关机时间（00:00–23:59）".into(),
            ));
        }
        if self.schedule_enabled && self.schedule_mode == ScheduleMode::Cron {
            for expression in [&self.start_cron, &self.stop_cron] {
                let cron = super::cron::Cron::parse(expression)
                    .map_err(|e| ApiError::BadRequest(e.into()))?;
                if cron
                    .occurrence(
                        apsp_protocol::now_timestamp(),
                        self.utc_offset_minutes,
                        true,
                    )
                    .is_none()
                {
                    return Err(ApiError::BadRequest(
                        "Cron 在未来五年内没有有效触发时间".into(),
                    ));
                }
            }
        }
        Ok(())
    }
    /// Whether the account's billed CDT usage this month reaches the policy's threshold;
    /// `None` when the bill may not drive automatic control.
    pub(crate) fn exceeded(
        &self,
        account: &Account,
        billing: &AccountBilling,
        now: i64,
    ) -> Option<bool> {
        if !account.enabled {
            return None;
        }
        Some(
            billing.usage(now)? as u128 * 100
                >= self.limit_gb as u128 * 1_000_000 * self.threshold_percent as u128,
        )
    }
    /// Whether the resource's traffic protection forbids starting it now.
    pub(crate) fn blocks_start(
        &self,
        account: &Account,
        billing: &AccountBilling,
        resource: &Automated,
        now: i64,
    ) -> bool {
        if self.enabled && self.cdt_guard.is_some() {
            return super::guard::exceeded(account, resource, now) != Some(false);
        }
        self.enabled
            && self.threshold_action == "stop"
            && (resource.automation.threshold_hold
                || self.exceeded(account, billing, now) != Some(false))
    }
    pub(crate) fn in_window(&self, now: i64) -> bool {
        if self.schedule_mode == ScheduleMode::Cron {
            let previous = |expression: &str| {
                super::cron::Cron::parse(expression).ok()?.occurrence(
                    now,
                    self.utc_offset_minutes,
                    false,
                )
            };
            return match (previous(&self.start_cron), previous(&self.stop_cron)) {
                (Some(start), Some(stop)) => start > stop,
                _ => false,
            };
        }
        let minute_now = (now + self.utc_offset_minutes * 60).rem_euclid(86400) / 60;
        let start = minute(&self.start_time).unwrap_or(0);
        let stop = minute(&self.stop_time).unwrap_or(0);
        if start < stop {
            minute_now >= start && minute_now < stop
        } else {
            minute_now >= start || minute_now < stop
        }
    }
    pub(crate) fn occurrence(&self, action: &str, now: i64) -> Option<i64> {
        if self.schedule_mode == ScheduleMode::Cron {
            let expression = if action == "start" {
                &self.start_cron
            } else {
                &self.stop_cron
            };
            let event = super::cron::Cron::parse(expression).ok()?.occurrence(
                now,
                self.utc_offset_minutes,
                false,
            )?;
            return (now - event < 600).then_some(event);
        }
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

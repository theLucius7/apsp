use super::super::{
    billing,
    client::Cloud,
    model::{Account, Resource},
};
use super::{State, jobs};
use crate::{
    cloud_api::Failure,
    error::{ApiError, ApiResult},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction, types::Json};

mod meter;
pub(super) mod seed;
#[cfg(test)]
mod tests;

pub(super) const MAX_BYTES: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct Config {
    pub threshold_bytes: u64,
    pub jump_bytes: u64,
    pub auto_start: bool,
}
impl Config {
    pub fn validate(&self) -> ApiResult<()> {
        if !(1..=MAX_BYTES).contains(&self.threshold_bytes)
            || !(1..=MAX_BYTES).contains(&self.jump_bytes)
        {
            return Err(ApiError::BadRequest(
                "CDT 停机线和突增阈值必须是有效的正整数字节数".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in super::super) struct Observation {
    pub billing_cycle: String,
    pub region: String,
    pub cloud_id: String,
    pub key_digest: String,
    pub account_revision: i64,
    pub max_usage_bytes: u64,
    pub max_jump_bytes: u64,
    pub queried_at: i64,
}

pub(super) fn key_digest(key: &str) -> String {
    format!("{:x}", Sha256::digest(key.as_bytes()))
}

pub(super) fn valid_cycle(cycle: &str) -> bool {
    let bytes = cycle.as_bytes();
    bytes.len() == 7
        && bytes[4] == b'-'
        && bytes[..4]
            .iter()
            .chain(bytes[5..].iter())
            .all(u8::is_ascii_digit)
        && cycle[..4].parse::<u32>().is_ok_and(|y| y >= 1970)
        && cycle[5..]
            .parse::<u32>()
            .is_ok_and(|m| (1..=12).contains(&m))
}

pub(super) fn observe(
    previous: Option<&Observation>,
    raw_bytes: u64,
    account: &Account,
    resource: &Resource,
    digest: &str,
    now: i64,
) -> Result<Observation, Failure> {
    let cycle = billing::month(now);
    let (mut usage, mut jump) = (raw_bytes, 0);
    if let Some(old) = previous {
        if !valid_cycle(&old.billing_cycle)
            || old.region != resource.region
            || old.cloud_id != resource.cloud_id
            || old.key_digest != digest
            || old.max_usage_bytes > MAX_BYTES
            || old.max_jump_bytes > old.max_usage_bytes
            || old.queried_at > now
            || old.billing_cycle > cycle
        {
            return Err("cdt_state_mismatch".into());
        }
        if old.billing_cycle == cycle {
            usage = usage.max(old.max_usage_bytes);
            jump = old
                .max_jump_bytes
                .max(raw_bytes.saturating_sub(old.max_usage_bytes));
        } else if old.max_usage_bytes > 0 && raw_bytes >= old.max_usage_bytes {
            return Err("cdt_reset_pending".into());
        }
    }
    if usage > MAX_BYTES {
        return Err("invalid_response".into());
    }
    Ok(Observation {
        billing_cycle: cycle,
        region: resource.region.clone(),
        cloud_id: resource.cloud_id.clone(),
        key_digest: digest.into(),
        account_revision: account.revision,
        max_usage_bytes: usage,
        max_jump_bytes: jump,
        queried_at: now,
    })
}

pub(super) fn exceeded(account: &Account, resource: &Resource, now: i64) -> Option<bool> {
    let config = resource.power_policy.cdt_guard.as_ref()?;
    let observation = resource.cdt_guard_state.as_ref()?;
    if !account.enabled
        || resource.cdt_guard_error.is_some()
        || observation.account_revision != account.revision
        || observation.region != resource.region
        || observation.cloud_id != resource.cloud_id
        || observation.billing_cycle != billing::month(now)
        || observation.queried_at <= 0
        || observation.queried_at > now
        || now - observation.queried_at > 120
    {
        return None;
    }
    let backlog = if observation.max_jump_bytes >= config.jump_bytes {
        observation.max_jump_bytes
    } else {
        0
    };
    Some(observation.max_usage_bytes as u128 + backlog as u128 >= config.threshold_bytes as u128)
}

pub(super) async fn poll(
    tx: &mut Transaction<'_, Postgres>,
    account: &Account,
    r: &mut Resource,
    state: &State,
    cloud: &Cloud,
    started_at: i64,
) -> ApiResult<()> {
    if !r.power_policy.enabled {
        return Ok(());
    }
    let reading = read(account, r, cloud, started_at).await;
    match reading {
        Ok(observation) => {
            // Persist the high-water mark in the same transaction as the durable intent.
            sqlx::query(
                "UPDATE alicloud_resources SET cdt_guard_state=$2,cdt_guard_error=NULL WHERE id=$1",
            )
            .bind(r.id)
            .bind(Json(&observation))
            .execute(&mut **tx)
            .await?;
            r.cdt_guard_state = Some(Json(observation));
            r.cdt_guard_error = None;
        }
        Err(error) => {
            sqlx::query("UPDATE alicloud_resources SET cdt_guard_error=$2,next_power_at=GREATEST(next_power_at,$3) WHERE id=$1")
                .bind(r.id).bind(error.code).bind(apsp_protocol::now_timestamp()+error.retry_after.max(60)).execute(&mut **tx).await?;
            sqlx::query("UPDATE alicloud_power_jobs SET status='cancelled',error_code='policy_inactive',updated_at=$2 WHERE resource_id=$1 AND action='start' AND status='queued'")
                .bind(r.id).bind(apsp_protocol::now_timestamp()).execute(&mut **tx).await?;
            return Ok(());
        }
    }
    let now = apsp_protocol::now_timestamp();
    let over = exceeded(account, r, now);
    let action = if over == Some(true) && state.status == "Running" {
        "stop"
    } else if over == Some(false)
        && state.status == "Stopped"
        && !r.manual_hold
        && r.power_policy
            .cdt_guard
            .as_ref()
            .is_some_and(|g| g.auto_start)
    {
        "start"
    } else {
        return Ok(());
    };
    if state.validate(action, "StopCharging").is_err() {
        return Ok(());
    }
    if let Err(error) = super::super::operations::idle(tx, r.id).await {
        return match error {
            ApiError::Conflict(_) => Ok(()),
            other => Err(other),
        };
    }
    jobs::prepare(
        tx,
        account,
        r,
        state,
        jobs::Intent {
            action,
            mode: "StopCharging",
            source: "cdt_guard",
            key: Some(format!("cdt-guard:{}:{}:{}", r.id, r.revision, now / 60)),
            expires_at: now + 90,
        },
        now,
    )
    .await?;
    Ok(())
}

pub(super) async fn read(
    account: &Account,
    r: &Resource,
    cloud: &Cloud,
    started_at: i64,
) -> Result<Observation, Failure> {
    match cloud.guard_usage(account, &r.region).await {
        Ok((bytes, digest)) => {
            let now = apsp_protocol::now_timestamp();
            if billing::month(now) != billing::month(started_at) {
                Err(Failure::from("cdt_month_changed"))
            } else {
                observe(
                    r.cdt_guard_state.as_ref().map(|v| &v.0),
                    bytes,
                    account,
                    r,
                    &digest,
                    now,
                )
            }
        }
        Err(error) => Err(error),
    }
}

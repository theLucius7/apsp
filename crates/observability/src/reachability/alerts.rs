//! Alerts for transitions into and out of a blocked state.
//!
//! Follows the path the expense reminders use: the event and its per-channel
//! deliveries live in this domain's tables and are rendered and sent through the
//! platform's channels. The event is recorded even when no channel is configured.

use super::{classify::Transition, targets::Target};
use crate::error::ApiResult;
use apsp_panel_host::{
    notifications::{delivery, webhook::Message},
    settings::{self, Settings as PanelSettings},
};
use apsp_protocol::now_timestamp;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

const MAX_ATTEMPTS: i32 = 5;
const DELIVERIES_PER_TICK: usize = 20;

fn tally(probes: &Value) -> BTreeMap<String, (u32, u32)> {
    let mut counts = BTreeMap::new();
    for probe in probes.as_array().map(Vec::as_slice).unwrap_or_default() {
        let carrier = probe["carrier"].as_str().unwrap_or("other").to_owned();
        let entry: &mut (u32, u32) = counts.entry(carrier).or_default();
        entry.1 += 1;
        if probe["connected"] == true {
            entry.0 += 1;
        }
    }
    counts
}

fn carrier_label(carrier: &str) -> &'static str {
    match carrier {
        "telecom" => "电信",
        "unicom" => "联通",
        "mobile" => "移动",
        _ => "其他",
    }
}

fn carrier_line(probes: &Value) -> String {
    let counts = tally(probes);
    ["telecom", "unicom", "mobile", "other"]
        .into_iter()
        .filter_map(|carrier| {
            let (connected, total) = counts.get(carrier)?;
            Some(format!("{} {connected}/{total}", carrier_label(carrier)))
        })
        .collect::<Vec<_>>()
        .join("、")
}

/// A short carrier breakdown. Addresses stay out of messages sent to third-party channels.
pub fn breakdown(detail: &Value) -> String {
    let china = carrier_line(&detail["china"]["probes"]);
    let mut parts = Vec::new();
    if !china.is_empty() {
        parts.push(format!("中国 {china}"));
    }
    let overseas = tally(&detail["overseas"]["probes"]);
    if !overseas.is_empty() {
        let (connected, total) = overseas
            .values()
            .fold((0, 0), |sum, count| (sum.0 + count.0, sum.1 + count.1));
        parts.push(format!("海外 {connected}/{total}"));
    }
    if let Some(port) = detail["control"]["port"].as_u64() {
        parts.push(format!(
            "对照端口 {port}：{}",
            carrier_line(&detail["control"]["probes"])
        ));
    }
    parts.join("；")
}

pub(super) async fn record(
    tx: &mut Transaction<'_, Postgres>,
    panel: &PanelSettings,
    target: &Target,
    target_id: i64,
    transition: Transition,
    detail: &Value,
    now: i64,
) -> ApiResult<()> {
    let (kind, title, event, from, to, message) = match transition {
        Transition::None => return Ok(()),
        Transition::Blocked { from, to } => (
            "blocked",
            "中国可达性告警",
            "reachability_blocked",
            from,
            to,
            format!(
                "节点 {} 从中国无法连接：{}。{}",
                target.node_names(),
                to.label(),
                breakdown(detail)
            ),
        ),
        Transition::Cleared { from, to } => (
            "cleared",
            "中国可达性恢复",
            "reachability_cleared",
            from,
            to,
            format!(
                "节点 {} 不再处于封锁状态，当前：{}。{}",
                target.node_names(),
                to.label(),
                breakdown(detail)
            ),
        ),
    };
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO singbox_reachability_events(id,target_id,server_id,kind,from_state,to_state,message,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(id)
    .bind(target_id)
    .bind(target.nodes.first().map(|node| node.server_id))
    .bind(kind)
    .bind(from.as_str())
    .bind(to.as_str())
    .bind(&message)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    let time: String = sqlx::query_scalar(
        "SELECT to_char(to_timestamp($1) AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS') || ' UTC'",
    )
    .bind(now as f64)
    .fetch_one(&mut **tx)
    .await?;
    let id_text = id.to_string();
    let server = target.server_names();
    for (channel, payload) in delivery::render(
        panel,
        &Message {
            title,
            server: &server,
            message: &message,
            time: &time,
            event,
            event_id: &id_text,
            category: "reachability",
        },
    ) {
        let (body, error) = match payload {
            Ok(body) => (body, None),
            Err(error) => (String::new(), Some(error)),
        };
        sqlx::query(
            "INSERT INTO singbox_reachability_deliveries(event_id,channel,payload,state,next_attempt_at,last_error,attempts) VALUES($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(id)
        .bind(channel)
        .bind(body)
        .bind(if error.is_some() { "failed" } else { "pending" })
        .bind(now)
        .bind(&error)
        .bind(if error.is_some() { MAX_ATTEMPTS } else { 0 })
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Sends what is due. Maintenance windows hold a delivery back like server alerts,
/// and a channel that was turned off cancels what it had pending.
pub(super) async fn deliver(pool: &PgPool) -> ApiResult<()> {
    let panel = settings::read(pool).await?;
    sqlx::query(
        "UPDATE singbox_reachability_deliveries SET state='cancelled' WHERE state IN ('pending','failed') AND attempts<$3 AND ((channel='telegram' AND NOT $1) OR (channel='webhook' AND NOT $2))",
    )
    .bind(panel.telegram_ready())
    .bind(panel.webhook_ready())
    .bind(MAX_ATTEMPTS)
    .execute(pool)
    .await?;
    for _ in 0..DELIVERIES_PER_TICK {
        let mut tx = pool.begin().await?;
        let now = now_timestamp();
        let row = sqlx::query(
            "SELECT d.id,d.channel,d.payload,d.attempts FROM singbox_reachability_deliveries d JOIN singbox_reachability_events e ON e.id=d.event_id \
             WHERE d.attempts<$2 AND ((d.state IN ('pending','failed') AND d.next_attempt_at<=$1) OR (d.state='sending' AND d.lease_until<$1)) \
             AND NOT EXISTS(SELECT 1 FROM operations_maintenance m WHERE e.server_id=ANY(m.targets) AND m.suppress_notifications AND m.starts_at<=$1 AND m.ends_at>$1) \
             AND NOT EXISTS(SELECT 1 FROM fleet_profiles p WHERE p.server_id=e.server_id AND p.lifecycle='maintenance' AND COALESCE(p.maintenance_from,0)<=$1 AND (p.maintenance_until IS NULL OR p.maintenance_until>$1)) \
             ORDER BY d.id LIMIT 1 FOR UPDATE OF d SKIP LOCKED",
        )
        .bind(now)
        .bind(MAX_ATTEMPTS)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            break;
        };
        let id: i64 = row.try_get("id")?;
        let attempts: i32 = row.try_get("attempts")?;
        sqlx::query(
            "UPDATE singbox_reachability_deliveries SET state='sending',attempts=attempts+1,lease_until=$2 WHERE id=$1",
        )
        .bind(id)
        .bind(now + 120)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        let channel: String = row.try_get("channel")?;
        let payload: String = row.try_get("payload")?;
        let (state, error, retry) = match delivery::send(&panel, &channel, &payload).await {
            Ok(()) => ("sent", None, 0),
            Err((error, after)) => (
                "failed",
                Some(error.chars().take(500).collect::<String>()),
                after.unwrap_or(30 * (1_i64 << attempts)).clamp(30, 86400),
            ),
        };
        sqlx::query(
            "UPDATE singbox_reachability_deliveries SET state=$2,last_error=$3,next_attempt_at=$4,lease_until=NULL WHERE id=$1 AND state='sending'",
        )
        .bind(id)
        .bind(state)
        .bind(error)
        .bind(now_timestamp() + retry)
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_breakdown_counts_each_carrier_without_addresses() {
        let detail = json!({
            "china":{"probes":[
                {"carrier":"telecom","connected":false},{"carrier":"telecom","connected":true},
                {"carrier":"unicom","connected":false},{"carrier":"mobile","connected":false},
            ]},
            "overseas":{"probes":[{"carrier":"other","connected":true},{"carrier":"other","connected":true}]},
            "control":{"status":"checked","port":22,"probes":[{"carrier":"telecom","connected":false}]},
        });
        assert_eq!(
            breakdown(&detail),
            "中国 电信 1/2、联通 0/1、移动 0/1；海外 2/2；对照端口 22：电信 0/1"
        );
        assert_eq!(breakdown(&json!({"reason":"no_public_address"})), "");
    }
}

//! Alerts for transitions into and out of a blocked state.
//!
//! Follows the path the expense reminders use: the event and its per-channel
//! deliveries live in this domain's tables, are rendered for the platform's channels and
//! delivered by the platform's deliverer. The event is recorded even when no channel is
//! configured.

use super::{classify::Transition, targets::Target};
use crate::{
    error::{ApiError, ApiResult},
    incidents,
};
use apsp_panel_host::{
    notifications::{
        Failure, delivery,
        queue::{self, Backoff, Claim, Claimed, Claiming, Claims, Outcome, Policy, Queue},
        webhook::Message,
    },
    ports::BoxFuture,
    settings::{self, Settings as PanelSettings},
};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Reachability alerts: leased one at a time and sent after the claim commits, up to
/// twenty per pass, five sends each. Messages held back by maintenance stay due.
const POLICY: Policy = Policy {
    table: "singbox_reachability_deliveries",
    claims: 20,
    claiming: Claiming::Leased {
        seconds: 120,
        batch: 1,
    },
    attempts: 5,
    backoff: Backoff::Doubling {
        first: 30,
        doublings: 4,
        min: 30,
        max: 86_400,
    },
    held: None,
};

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
        .bind(if error.is_some() { POLICY.attempts } else { 0 })
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// The reachability alerts due for the channels.
pub(super) struct Deliveries<'a> {
    pool: &'a PgPool,
    panel: &'a PanelSettings,
}

pub(super) struct Delivery {
    id: i64,
    channel: String,
    payload: String,
}

impl Queue for Deliveries<'_> {
    const POLICY: Policy = POLICY;
    type Message = Delivery;
    type Error = ApiError;

    fn claim(&self, claim: Claim) -> Claims<'_, Delivery, ApiError> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            let rows = sqlx::query(&format!(
                "SELECT d.id,d.channel,d.payload,d.attempts FROM singbox_reachability_deliveries d JOIN singbox_reachability_events e ON e.id=d.event_id \
                 WHERE d.attempts<$2 AND ((d.state IN ('pending','failed') AND d.next_attempt_at<=$1) OR (d.state='sending' AND d.lease_until<$1)) \
                 AND NOT {} \
                 ORDER BY d.id LIMIT $3 FOR UPDATE OF d SKIP LOCKED",
                incidents::notifications_held("e.server_id", "$1")
            ))
            .bind(claim.now)
            .bind(Self::POLICY.attempts)
            .bind(claim.limit)
            .fetch_all(&mut *tx)
            .await?;
            let mut claimed = Vec::new();
            for row in rows {
                let id: i64 = row.try_get("id")?;
                sqlx::query(
                    "UPDATE singbox_reachability_deliveries SET state='sending',attempts=attempts+1,lease_until=$2 WHERE id=$1",
                )
                .bind(id)
                .bind(claim.lease_until)
                .execute(&mut *tx)
                .await?;
                claimed.push(Claimed {
                    message: Delivery {
                        id,
                        channel: row.try_get("channel")?,
                        payload: row.try_get("payload")?,
                    },
                    attempts: row.try_get("attempts")?,
                });
            }
            tx.commit().await?;
            Ok(claimed)
        })
    }

    fn send<'a>(&'a self, alert: &'a Delivery) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(delivery::send(self.panel, &alert.channel, &alert.payload))
    }

    fn settle(&self, alert: Delivery, outcome: Outcome) -> BoxFuture<'_, ApiResult<()>> {
        Box::pin(async move {
            let (state, next) = if outcome.sent() {
                ("sent", outcome.finished_at)
            } else {
                ("failed", outcome.retry_at)
            };
            sqlx::query(
                "UPDATE singbox_reachability_deliveries SET state=$2,last_error=$3,next_attempt_at=$4,lease_until=NULL WHERE id=$1 AND state='sending'",
            )
            .bind(alert.id)
            .bind(state)
            .bind(&outcome.error)
            .bind(next)
            .execute(self.pool)
            .await?;
            Ok(())
        })
    }
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
    .bind(POLICY.attempts)
    .execute(pool)
    .await?;
    queue::deliver(&Deliveries {
        pool,
        panel: &panel,
    })
    .await
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

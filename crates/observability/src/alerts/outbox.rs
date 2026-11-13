//! The alert outbox: one message per alert event, kind and ready channel, queued when the
//! event opens or recovers and delivered by the platform's deliverer. The platform's
//! settings and channel pages reach it through `notifications::outbox`, which the
//! application connects to the functions at the end of this module.

use crate::{
    error::ApiResult,
    incidents,
    notifications::{
        Failure, delivery,
        outbox::ChannelDeliveries,
        queue::{self, Backoff, Claim, Claimed, Claiming, Claims, Outcome, Policy, Queue},
    },
    settings::{self, Settings},
};
use apsp_panel_host::ports::BoxFuture;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};

/// Alert messages: each channel claims one message at a time and keeps it locked, with the
/// settings it is sent with, until the outcome is recorded; up to four per channel and
/// pass, eight sends each. Messages held back by maintenance stay due.
const POLICY: Policy = Policy {
    table: "notification_outbox",
    claims: 4,
    claiming: Claiming::Locked,
    attempts: 8,
    backoff: Backoff::Doubling {
        first: 30,
        doublings: 7,
        min: 1,
        max: 86_400,
    },
    held: None,
};

/// Sends one rendered message through a channel; tests stand in for the channels.
type Sender<'a> =
    dyn Fn(Settings, String, String) -> BoxFuture<'static, Result<(), Failure>> + Sync + 'a;
type Clock<'a> = dyn Fn() -> i64 + Sync + 'a;

/// The alert messages of one channel.
pub(crate) struct Outbox<'a> {
    pool: &'a PgPool,
    channel: &'static str,
    send: &'a Sender<'a>,
    clock: &'a Clock<'a>,
}

pub(crate) struct Message {
    tx: Transaction<'static, Postgres>,
    settings: Settings,
    id: i64,
    text: String,
}

impl Queue for Outbox<'_> {
    const POLICY: Policy = POLICY;
    type Message = Message;
    type Error = anyhow::Error;

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn claim(&self, claim: Claim) -> Claims<'_, Message, anyhow::Error> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            // Hold the shared settings lock through the bounded request so credentials cannot
            // change between checking the destination and attempting delivery.
            let settings = settings::share(&mut tx).await?;
            if !settings.telegram_ready() && !settings.webhook_ready() {
                return Ok(Vec::new());
            }
            let row: Option<(i64,String,i32)> = sqlx::query_as(&format!(
                "SELECT o.id,o.message,o.attempts FROM notification_outbox o
                 JOIN server_alert_events e ON e.id=o.event_id JOIN servers s ON s.id=e.server_id
                 WHERE o.status='pending' AND o.next_attempt_at<=$1 AND s.deleted_at IS NULL AND o.channel=$5
                 AND NOT {}
                 AND (e.category<>'offline' OR ($2 AND {}))
                 AND ((o.channel='telegram' AND $3) OR (o.channel='webhook' AND $4))
                 AND NOT EXISTS(SELECT 1 FROM notification_outbox earlier WHERE earlier.event_id=o.event_id AND earlier.channel=o.channel AND earlier.id<o.id AND earlier.status='pending')
                 ORDER BY o.next_attempt_at,o.id LIMIT 1 FOR UPDATE OF o SKIP LOCKED",
                incidents::notifications_held("s.id", "$1"),
                crate::server_assets::sql::offline_notify("s")
            ))
            .bind(claim.now).bind(settings.offline_alerts).bind(settings.telegram_ready()).bind(settings.webhook_ready()).bind(self.channel).fetch_optional(&mut *tx).await?;
            Ok(row
                .map(|(id, text, attempts)| Claimed {
                    message: Message {
                        tx,
                        settings,
                        id,
                        text,
                    },
                    attempts,
                })
                .into_iter()
                .collect())
        })
    }

    fn send<'a>(&'a self, message: &'a Message) -> BoxFuture<'a, Result<(), Failure>> {
        (self.send)(
            message.settings.clone(),
            self.channel.to_owned(),
            message.text.clone(),
        )
    }

    fn settle(&self, message: Message, outcome: Outcome) -> BoxFuture<'_, anyhow::Result<()>> {
        Box::pin(async move {
            let Message { mut tx, id, .. } = message;
            let (status, next) = if outcome.sent() {
                ("sent", outcome.finished_at)
            } else if outcome.last_attempt {
                ("failed", outcome.retry_at)
            } else {
                ("pending", outcome.retry_at)
            };
            sqlx::query("UPDATE notification_outbox SET status=$2,last_error=$3,attempts=attempts+1,next_attempt_at=$4,last_attempt_at=$5,delivered_at=CASE WHEN $2='sent' THEN $6 ELSE NULL END WHERE id=$1")
                .bind(id).bind(status).bind(&outcome.error).bind(next).bind(outcome.attempted_at).bind(outcome.finished_at).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(())
        })
    }
}

/// Sends the due alert messages, each channel on its own so that a slow channel does not
/// hold up the other. The time never goes back before the maintenance pass's `now`.
pub async fn dispatch(pool: &PgPool, now: i64) -> anyhow::Result<()> {
    let send = |settings: Settings,
                channel: String,
                message: String|
     -> BoxFuture<'static, Result<(), Failure>> {
        Box::pin(async move { delivery::send(&settings, &channel, &message).await })
    };
    let clock = move || apsp_protocol::now_timestamp().max(now);
    deliver(pool, &send, &clock).await
}

async fn deliver(pool: &PgPool, send: &Sender<'_>, clock: &Clock<'_>) -> anyhow::Result<()> {
    let outbox = |channel| Outbox {
        pool,
        channel,
        send,
        clock,
    };
    let (telegram, webhook) = (outbox("telegram"), outbox("webhook"));
    tokio::try_join!(queue::deliver(&telegram), queue::deliver(&webhook))?;
    Ok(())
}

/// Drops the waiting messages that a settings change made stale; the platform's settings
/// pages call this through `notifications::outbox` in the transaction that saves it.
pub async fn settings_changed(
    tx: &mut Transaction<'_, Postgres>,
    old: &Settings,
    new: &Settings,
) -> ApiResult<()> {
    // Do not deliver stale notifications to a newly configured recipient.
    let telegram = !new.telegram_ready()
        || old.telegram_chat_id != new.telegram_chat_id
        || old.telegram_token != new.telegram_token
        || old.telegram_thread_id != new.telegram_thread_id
        || old.telegram_template != new.telegram_template;
    let webhook =
        !new.webhook.as_ref().is_some_and(|config| config.enabled) || old.webhook != new.webhook;
    for (channel, stale) in [("telegram", telegram), ("webhook", webhook)] {
        if stale {
            sqlx::query("UPDATE notification_outbox SET status='cancelled' WHERE channel=$1 AND status='pending'")
                .bind(channel).execute(&mut **tx).await?;
        }
    }
    if !new.notification_enabled {
        sqlx::query("UPDATE notification_outbox SET status='cancelled' WHERE status='pending'")
            .execute(&mut **tx)
            .await?;
    }
    for (category, enabled) in [
        ("offline", new.offline_alerts),
        ("expiry", new.expiry_alert_days > 0),
        ("traffic", new.traffic_alert_percentage > 0),
    ] {
        if !enabled {
            sqlx::query("UPDATE notification_outbox o SET status='cancelled' FROM server_alert_events e WHERE o.event_id=e.id AND e.category=$1 AND o.status='pending'")
                .bind(category).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

/// Counts by state and the latest attempt of one channel, for the channel page.
pub async fn deliveries(
    tx: &mut Transaction<'_, Postgres>,
    channel: &str,
) -> ApiResult<ChannelDeliveries> {
    let counts: Value = sqlx::query_scalar("SELECT jsonb_build_object('pending',COUNT(*) FILTER(WHERE status='pending'),'failed',COUNT(*) FILTER(WHERE status='failed'),'sent',COUNT(*) FILTER(WHERE status='sent'),'next_attempt_at',MIN(next_attempt_at) FILTER(WHERE status='pending')) FROM notification_outbox WHERE channel=$1")
        .bind(channel).fetch_one(&mut **tx).await?;
    let last: Option<Value> = sqlx::query_scalar("SELECT jsonb_build_object('status',status,'attempts',attempts,'last_error',last_error,'attempted_at',last_attempt_at,'delivered_at',delivered_at) FROM notification_outbox WHERE channel=$1 AND last_attempt_at IS NOT NULL ORDER BY last_attempt_at DESC,id DESC LIMIT 1")
        .bind(channel).fetch_optional(&mut **tx).await?;
    Ok(ChannelDeliveries { counts, last })
}

/// Messages still waiting for any channel, for the system health view.
pub async fn waiting(pool: &PgPool) -> ApiResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM notification_outbox WHERE status='pending'")
            .fetch_one(pool)
            .await?,
    )
}

#[cfg(test)]
#[path = "outbox_tests.rs"]
mod tests;

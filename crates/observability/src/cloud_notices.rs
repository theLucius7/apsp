//! Cloud resource notices: an event per notice and a message per ready channel, delivered
//! by the platform's deliverer.

use crate::{
    error::{ApiError, ApiResult},
    notifications::{
        Failure, delivery,
        queue::{self, Backoff, Claim, Claimed, Claiming, Claims, Outcome, Policy, Queue},
        webhook,
    },
    settings::Settings,
};
use apsp_panel_host::ports::BoxFuture;
use apsp_resources::cloud::model::Resource;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};

/// Cloud notices: each channel claims one message per pass and keeps it locked, with the
/// settings it is sent with, until the outcome is recorded; eight sends each.
const POLICY: Policy = Policy {
    table: "alicloud_deliveries",
    claims: 1,
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

pub async fn record(
    tx: &mut Transaction<'_, Postgres>,
    resource: &Resource,
    key: &str,
    title: &str,
    message: &str,
    now: i64,
) -> ApiResult<()> {
    let id:Option<i64>=sqlx::query_scalar("INSERT INTO alicloud_events(resource_id,dedup_key,title,message,created_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT(dedup_key) DO NOTHING RETURNING id")
        .bind(resource.id).bind(key).bind(title).bind(message).bind(now).fetch_optional(&mut **tx).await?;
    let Some(id) = id else {
        return Ok(());
    };
    let value: Option<Value> =
        sqlx::query_scalar("SELECT settings FROM panel_settings WHERE singleton")
            .fetch_optional(&mut **tx)
            .await?;
    let Some(value) = value else {
        return Ok(());
    };
    let settings: Settings = serde_json::from_value(value).map_err(anyhow::Error::from)?;
    let time = apsp_cloud_api::signing::iso_time(now);
    let event_id = format!("alicloud:{id}");
    let message = webhook::Message {
        title,
        server: &resource.name,
        message,
        time: &time,
        event: "alicloud",
        event_id: &event_id,
        category: "alicloud",
    };
    for (channel, payload) in delivery::render(&settings, &message) {
        let (payload, status, error) = match payload {
            Ok(value) => (value, "pending", None),
            Err(error) => (String::new(), "failed", Some(error)),
        };
        sqlx::query("INSERT INTO alicloud_deliveries(event_id,channel,payload,status,last_error,next_attempt_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING").bind(id).bind(channel).bind(payload).bind(status).bind(error).bind(now).execute(&mut **tx).await?;
    }
    Ok(())
}

/// Sends one rendered message through a channel; tests stand in for the channels.
type Sender<'a> =
    dyn Fn(Settings, String, String) -> BoxFuture<'static, Result<(), Failure>> + Sync + 'a;

/// The notices of one channel.
pub(crate) struct Notices<'a> {
    pool: &'a PgPool,
    channel: &'static str,
    send: &'a Sender<'a>,
}

pub(crate) struct Notice {
    tx: Transaction<'static, Postgres>,
    settings: Settings,
    id: i64,
    payload: String,
}

impl Queue for Notices<'_> {
    const POLICY: Policy = POLICY;
    type Message = Notice;
    type Error = ApiError;

    fn claim(&self, claim: Claim) -> Claims<'_, Notice, ApiError> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await?;
            let value: Option<Value> =
                sqlx::query_scalar("SELECT settings FROM panel_settings WHERE singleton FOR SHARE")
                    .fetch_optional(&mut *tx)
                    .await?;
            let Some(value) = value else {
                return Ok(Vec::new());
            };
            let settings: Settings = serde_json::from_value(value).map_err(anyhow::Error::from)?;
            if (self.channel == "telegram" && !settings.telegram_ready())
                || (self.channel == "webhook" && !settings.webhook_ready())
            {
                return Ok(Vec::new());
            }
            let row:Option<(i64,String,i32)>=sqlx::query_as("SELECT d.id,d.payload,d.attempts FROM alicloud_deliveries d WHERE d.status='pending' AND d.channel=$1 AND d.next_attempt_at<=$2 ORDER BY d.next_attempt_at,d.id LIMIT 1 FOR UPDATE OF d SKIP LOCKED").bind(self.channel).bind(claim.now).fetch_optional(&mut *tx).await?;
            Ok(row
                .map(|(id, payload, attempts)| Claimed {
                    message: Notice {
                        tx,
                        settings,
                        id,
                        payload,
                    },
                    attempts,
                })
                .into_iter()
                .collect())
        })
    }

    fn send<'a>(&'a self, notice: &'a Notice) -> BoxFuture<'a, Result<(), Failure>> {
        (self.send)(
            notice.settings.clone(),
            self.channel.to_owned(),
            notice.payload.clone(),
        )
    }

    fn settle(&self, notice: Notice, outcome: Outcome) -> BoxFuture<'_, ApiResult<()>> {
        Box::pin(async move {
            let Notice { mut tx, id, .. } = notice;
            let (status, next) = if outcome.sent() {
                ("sent", outcome.finished_at)
            } else if outcome.last_attempt {
                ("failed", outcome.retry_at)
            } else {
                ("pending", outcome.retry_at)
            };
            sqlx::query("UPDATE alicloud_deliveries SET status=$2,last_error=$3,attempts=attempts+1,next_attempt_at=$4,delivered_at=CASE WHEN $2='sent' THEN $5 ELSE NULL END WHERE id=$1").bind(id).bind(status).bind(&outcome.error).bind(next).bind(outcome.finished_at).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(())
        })
    }
}

pub async fn dispatch(pool: &PgPool) -> ApiResult<()> {
    dispatch_with(pool, |settings, channel, payload| async move {
        delivery::send(&settings, &channel, &payload).await
    })
    .await
}

/// Sends the due notices of each channel in turn, with `send` standing in for the channels.
pub async fn dispatch_with<F, Fut>(pool: &PgPool, send: F) -> ApiResult<()>
where
    F: Fn(Settings, String, String) -> Fut + Sync,
    Fut: std::future::Future<Output = Result<(), Failure>> + Send + 'static,
{
    let send = move |settings: Settings,
                     channel: String,
                     payload: String|
          -> BoxFuture<'static, Result<(), Failure>> {
        Box::pin(send(settings, channel, payload))
    };
    for channel in ["telegram", "webhook"] {
        queue::deliver(&Notices {
            pool,
            channel,
            send: &send,
        })
        .await?;
    }
    Ok(())
}

/// Drops notification events created before `before`.
pub async fn prune(pool: &PgPool, before: i64) -> ApiResult<()> {
    sqlx::query("DELETE FROM alicloud_events WHERE created_at<$1")
        .bind(before)
        .execute(pool)
        .await?;
    Ok(())
}

/// The latest notification events with their delivery attempts, newest first.
pub async fn recent(pool: &PgPool) -> ApiResult<Vec<Value>> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('id',e.id,'resource_id',e.resource_id,'title',e.title,'message',e.message,'created_at',e.created_at,'deliveries',COALESCE((SELECT jsonb_agg(jsonb_build_object('channel',d.channel,'status',d.status,'attempts',d.attempts,'last_error',d.last_error,'next_attempt_at',d.next_attempt_at,'delivered_at',d.delivered_at) ORDER BY d.id) FROM alicloud_deliveries d WHERE d.event_id=e.id),'[]'::jsonb)) FROM alicloud_events e ORDER BY e.id DESC LIMIT 100").fetch_all(pool).await?)
}

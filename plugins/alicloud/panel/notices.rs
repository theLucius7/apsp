use super::model::Resource;
use crate::{
    error::ApiResult,
    notifications::{plugin, webhook},
    settings::Settings,
};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};

pub(super) async fn record(
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
    let time = crate::cloud_api::signing::iso_time(now);
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
    for (channel, payload) in plugin::render(&settings, &message) {
        let (payload, status, error) = match payload {
            Ok(value) => (value, "pending", None),
            Err(error) => (String::new(), "failed", Some(error)),
        };
        sqlx::query("INSERT INTO alicloud_deliveries(event_id,channel,payload,status,last_error,next_attempt_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING").bind(id).bind(channel).bind(payload).bind(status).bind(error).bind(now).execute(&mut **tx).await?;
    }
    Ok(())
}
pub(super) async fn dispatch(pool: &PgPool) -> ApiResult<()> {
    dispatch_with(pool, |settings, channel, payload| async move {
        plugin::send(&settings, &channel, &payload).await
    })
    .await
}
pub(super) async fn dispatch_with<F, Fut>(pool: &PgPool, mut send: F) -> ApiResult<()>
where
    F: FnMut(Settings, String, String) -> Fut,
    Fut: std::future::Future<Output = Result<(), (String, Option<i64>)>>,
{
    for channel in ["telegram", "webhook"] {
        let mut tx = pool.begin().await?;
        let value: Option<Value> =
            sqlx::query_scalar("SELECT settings FROM panel_settings WHERE singleton FOR SHARE")
                .fetch_optional(&mut *tx)
                .await?;
        let Some(value) = value else {
            return Ok(());
        };
        let settings: Settings = serde_json::from_value(value).map_err(anyhow::Error::from)?;
        if (channel == "telegram" && !settings.telegram_ready())
            || (channel == "webhook" && !settings.webhook_ready())
        {
            continue;
        }
        let now = apsp_protocol::now_timestamp();
        let row:Option<(i64,String,i32)>=sqlx::query_as("SELECT d.id,d.payload,d.attempts FROM alicloud_deliveries d WHERE d.status='pending' AND d.channel=$1 AND d.next_attempt_at<=$2 ORDER BY d.next_attempt_at,d.id LIMIT 1 FOR UPDATE OF d SKIP LOCKED").bind(channel).bind(now).fetch_optional(&mut *tx).await?;
        let Some((id, payload, attempts)) = row else {
            continue;
        };
        let (status, error, delay) = match send(settings, channel.into(), payload).await {
            Ok(()) => ("sent", None, 0),
            Err((message, retry)) => (
                if attempts >= 7 { "failed" } else { "pending" },
                Some(message),
                retry
                    .unwrap_or(30 * (1_i64 << attempts.clamp(0, 7)))
                    .clamp(1, 86400),
            ),
        };
        let completed = apsp_protocol::now_timestamp();
        sqlx::query("UPDATE alicloud_deliveries SET status=$2,last_error=$3,attempts=attempts+1,next_attempt_at=$4,delivered_at=CASE WHEN $2='sent' THEN $5 ELSE NULL END WHERE id=$1").bind(id).bind(status).bind(error).bind(completed+delay).bind(completed).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    Ok(())
}

/// Drops notification events created before `before`.
pub(super) async fn prune(pool: &PgPool, before: i64) -> ApiResult<()> {
    sqlx::query("DELETE FROM alicloud_events WHERE created_at<$1")
        .bind(before)
        .execute(pool)
        .await?;
    Ok(())
}

/// The latest notification events with their delivery attempts, newest first.
pub(super) async fn recent(pool: &PgPool) -> ApiResult<Vec<Value>> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('id',e.id,'resource_id',e.resource_id,'title',e.title,'message',e.message,'created_at',e.created_at,'deliveries',COALESCE((SELECT jsonb_agg(jsonb_build_object('channel',d.channel,'status',d.status,'attempts',d.attempts,'last_error',d.last_error,'next_attempt_at',d.next_attempt_at,'delivered_at',d.delivered_at) ORDER BY d.id) FROM alicloud_deliveries d WHERE d.event_id=e.id),'[]'::jsonb)) FROM alicloud_events e ORDER BY e.id DESC LIMIT 100").fetch_all(pool).await?)
}

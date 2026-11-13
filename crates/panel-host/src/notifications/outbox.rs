//! The alert outbox the channels serve (typed interface, ADR 0128). Observability keeps
//! it: the alert events and the message queued for each event and channel, delivered by
//! [`super::queue`]. The settings and channel pages reach it here, and the application
//! connects observability's answer.
//!
//! Without a connection these calls fail with the transaction they belong to, so a
//! settings change never leaves stale messages for a new recipient.

use crate::{
    error::{ApiError, ApiResult},
    ports::{BoxFuture, Port},
    settings::Settings,
};
use serde_json::Value;
use sqlx::{Postgres, Transaction};

/// What the channel page shows of the messages queued for one channel.
pub struct ChannelDeliveries {
    /// Messages by state, with the earliest time a waiting one is due.
    pub counts: Value,
    /// The latest attempt, if there was one.
    pub last: Option<Value>,
}

pub trait Outbox: Send + Sync {
    /// Drops the waiting messages a change of the notification settings made stale: those
    /// of a channel turned off or given another recipient or format, all of them when
    /// notifications are turned off, and those of a kind of alert turned off. Runs in the
    /// transaction that saves the change.
    fn settings_changed<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        old: &'a Settings,
        new: &'a Settings,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// The messages queued for one channel, as its status shows them.
    fn deliveries<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        channel: &'a str,
    ) -> BoxFuture<'a, ApiResult<ChannelDeliveries>>;
}

static OUTBOX: Port<dyn Outbox> = Port::new();

pub fn install(outbox: &'static dyn Outbox) {
    OUTBOX.install(outbox);
}

fn required() -> ApiResult<&'static dyn Outbox> {
    OUTBOX.get().ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!(
            "the alert outbox is not connected to the notification settings"
        ))
    })
}

pub async fn settings_changed(
    tx: &mut Transaction<'_, Postgres>,
    old: &Settings,
    new: &Settings,
) -> ApiResult<()> {
    required()?.settings_changed(tx, old, new).await
}

pub async fn deliveries(
    tx: &mut Transaction<'_, Postgres>,
    channel: &str,
) -> ApiResult<ChannelDeliveries> {
    required()?.deliveries(tx, channel).await
}

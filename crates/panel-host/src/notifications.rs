//! Notification delivery: the Telegram and Webhook channels and their templates, and the
//! deliverer every queue of messages runs on (`queue`). Alert rules, their events and the
//! alert outbox belong to observability; the settings and channel pages reach the outbox
//! through `outbox`.

pub mod channels;
pub mod delivery;
pub mod outbox;
pub mod queue;
mod telegram;
pub mod template;
pub mod webhook;
pub mod webhook_settings;
use crate::{http::RetryAfter, settings::Settings};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
pub use telegram::Failure;
pub use template::valid as valid_template;

/// How long a channel may make the next delivery attempt wait: one second to a day.
const RETRY_AFTER: RetryAfter = RetryAfter::new(1, 86_400);

/// The panel settings, locked for the transaction that changes notification state.
pub async fn lock_settings(tx: &mut Transaction<'_, Postgres>) -> anyhow::Result<Settings> {
    let value: Value =
        sqlx::query_scalar("SELECT settings FROM panel_settings WHERE singleton FOR UPDATE")
            .fetch_one(&mut **tx)
            .await?;
    Ok(serde_json::from_value(value)?)
}

/// The panel settings, held shared for the transaction that sends with them, so that the
/// credentials cannot change between checking a channel and sending through it.
pub async fn share_settings(tx: &mut Transaction<'_, Postgres>) -> anyhow::Result<Settings> {
    let value: Value =
        sqlx::query_scalar("SELECT settings FROM panel_settings WHERE singleton FOR SHARE")
            .fetch_one(&mut **tx)
            .await?;
    Ok(serde_json::from_value(value)?)
}

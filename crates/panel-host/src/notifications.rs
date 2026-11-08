//! Notification delivery: the outbox, the Telegram and Webhook channels and their
//! templates. Alert rules, their evaluation and the alert events belong to observability,
//! which queues messages here.

pub mod channels;
mod outbox;
pub mod plugin;
mod retry;
mod telegram;
pub mod template;
pub mod webhook;
pub mod webhook_settings;
use crate::settings::Settings;
pub use outbox::dispatch;
use serde_json::Value;
use sqlx::{Postgres, Transaction};
pub use template::valid as valid_template;

/// The panel settings, locked for the transaction that changes notification state.
pub async fn lock_settings(tx: &mut Transaction<'_, Postgres>) -> anyhow::Result<Settings> {
    let value: Value =
        sqlx::query_scalar("SELECT settings FROM panel_settings WHERE singleton FOR UPDATE")
            .fetch_one(&mut **tx)
            .await?;
    Ok(serde_json::from_value(value)?)
}

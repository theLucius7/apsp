//! Notification delivery: the outbox, the Telegram and Webhook channels and their
//! templates. Alert rules, evaluation and events belong to observability.

pub mod channels;
mod evaluation;
pub mod events;
mod outbox;
pub mod plugin;
mod resources;
mod retry;
pub mod rules;
mod telegram;
mod template;
pub mod webhook;
pub mod webhook_settings;
use crate::settings::Settings;
pub use evaluation::evaluate;
pub use outbox::dispatch;
use serde_json::Value;
use sqlx::{Postgres, Transaction};
pub use template::valid as valid_template;

pub(super) async fn lock_settings(tx: &mut Transaction<'_, Postgres>) -> anyhow::Result<Settings> {
    let value: Value =
        sqlx::query_scalar("SELECT settings FROM panel_settings WHERE singleton FOR UPDATE")
            .fetch_one(&mut **tx)
            .await?;
    Ok(serde_json::from_value(value)?)
}

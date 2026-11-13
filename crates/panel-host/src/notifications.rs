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
use crate::http::RetryAfter;
pub use telegram::Failure;
pub use template::valid as valid_template;

/// How long a channel may make the next delivery attempt wait: one second to a day.
const RETRY_AFTER: RetryAfter = RetryAfter::new(1, 86_400);

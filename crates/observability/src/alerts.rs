//! Alert rules, their evaluation against telemetry and traffic, the alert events, and the
//! outbox that queues their messages for the platform's notification channels.

mod evaluation;
pub mod events;
pub mod outbox;
mod resources;
pub mod rules;

pub use evaluation::evaluate;
pub use outbox::dispatch;

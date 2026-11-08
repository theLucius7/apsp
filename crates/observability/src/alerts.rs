//! Alert rules, their evaluation against telemetry and traffic, and the alert events
//! that the platform's notification channels deliver.

mod evaluation;
pub mod events;
mod resources;
pub mod rules;

pub use evaluation::evaluate;

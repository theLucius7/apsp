//! Nexus compatibility on the access side (ADR 0115, ADR 0131): the bridge that keeps
//! migrated members' subscriptions and usage with the retained renderer, and the device
//! sessions behind Nexus device limits. Everything that knows the bridge is in `bridge`,
//! so the bridge exits with this module (roadmap 1.5).

pub mod bridge;
pub mod sessions;

//! Nexus compatibility on the access side (ADR 0115, ADR 0131, ADR 0140): the bridge that
//! keeps un-adopted migrated members' subscriptions and usage with the retained renderer,
//! native adoption, and the device sessions behind Nexus device limits.

pub mod adoption;
pub mod bridge;
pub mod sessions;

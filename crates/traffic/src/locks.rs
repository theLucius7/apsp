//! The traffic service's advisory lock keys (the rules are in `apsp_panel_host::locks`).

use apsp_panel_host::locks::Key;

/// Publication and every change to what it compiles: group topology, assignments, nodes and
/// server configuration ([`crate::business::lock_publication`]). The baseline function
/// `singbox_nexus_bridge_exchange` takes it too.
pub const PUBLICATION: Key = Key::new(73201001, "publication");
/// The baseline function `singbox_nexus_node_status_exchange`, which stores the legacy node
/// status the compatibility bridge sends, one exchange at a time.
pub const NODE_STATUS_EXCHANGE: Key = Key::new(73201003, "Nexus node status exchange");
/// Importing the Nexus node inventory, in full or appended: one inventory writer at a time.
pub const NODE_INVENTORY: Key = Key::new(71410071, "Nexus node inventory import");
/// Subscription source mutation receipts, one object per request: the request id's first
/// four bytes ([`Key::offset`]).
pub const SOURCE_REQUESTS: Key = Key::pair(73402901, 0, "subscription source request");
/// Adding subscription sources, directly or from a preview, within the source limit.
pub const SOURCE_CREATION: Key = Key::pair(73402902, 1, "subscription source creation");
/// Claiming due subscription source refreshes, across panel processes.
pub const SOURCE_REFRESHES: Key = Key::pair(73402903, 1, "subscription source refresh");
/// Storing subscription source previews, within each administrator's preview limit.
pub const SOURCE_PREVIEWS: Key = Key::pair(73402904, 1, "subscription source preview");

/// Every key above, for the application's check of all declarations.
pub const KEYS: &[Key] = &[
    PUBLICATION,
    NODE_STATUS_EXCHANGE,
    NODE_INVENTORY,
    SOURCE_REQUESTS,
    SOURCE_CREATION,
    SOURCE_REFRESHES,
    SOURCE_PREVIEWS,
];

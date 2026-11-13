//! Proxy access's advisory lock keys (the rules are in `apsp_panel_host::locks`).

use apsp_panel_host::locks::Key;

/// The baseline function `singbox_nexus_device_exchange`, which stores the device limits and
/// addresses the compatibility bridge sends, one exchange at a time.
pub const DEVICE_EXCHANGE: Key = Key::new(73201002, "Nexus device exchange");

/// Every key above, for the application's check of all declarations.
pub const KEYS: &[Key] = &[DEVICE_EXCHANGE];

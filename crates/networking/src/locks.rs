//! Basic networking's advisory lock keys (the rules are in `apsp_panel_host::locks`).

use apsp_panel_host::locks::TextKey;

/// One forwarding listener the panel owns, named by [`forwarding_listener`], so that two
/// documents cannot claim the same server, protocol and port.
pub const FORWARDING_LISTENER: TextKey = TextKey::new(0, "forwarding listener");

/// Every text key above, for the application's check of all declarations.
pub const TEXT_KEYS: &[TextKey] = &[FORWARDING_LISTENER];

/// The text that names a forwarding listener's lock.
pub fn forwarding_listener(server_id: i64, protocol: &str, port: u16) -> String {
    format!("apsp-forward:{server_id}:{protocol}:{port}")
}

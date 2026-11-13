//! The names and certificates domain's advisory lock keys (the rules are in
//! `apsp_panel_host::locks`).

use apsp_panel_host::locks::Key;

/// Adding DDNS rules, within the rule limit.
pub const DDNS_RULES: Key = Key::new(739104823, "DDNS rule creation");
/// Writing DNS records: DDNS lease claims and record writes, moving rules to another server,
/// rolling a rule back, and changing, rolling back or reconciling records by hand all take
/// this lock first.
pub const DNS_WRITES: Key = Key::new(739104824, "DNS writes");

/// Every key above, for the application's check of all declarations.
pub const KEYS: &[Key] = &[DDNS_RULES, DNS_WRITES];

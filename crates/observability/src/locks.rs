//! The observability domain's advisory lock keys (the rules are in `apsp_panel_host::locks`).

use apsp_panel_host::locks::Key;

/// Latency task defaults and group edits, taken before server row locks
/// ([`crate::latency_tasks::lock`]).
pub const LATENCY_TASKS: Key = Key::pair(1936289389, 1, "latency tasks");

/// Every key above, for the application's check of all declarations.
pub const KEYS: &[Key] = &[LATENCY_TASKS];

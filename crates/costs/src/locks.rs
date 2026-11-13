//! The costs domain's advisory lock keys (the rules are in `apsp_panel_host::locks`).

use apsp_panel_host::locks::Key;

/// Changing expenses: recurring services, payments, labels, settings, imports and reminder
/// delivery all take this lock first.
pub const EXPENSES: Key = Key::new(73201067, "expenses");

/// Every key above, for the application's check of all declarations.
pub const KEYS: &[Key] = &[EXPENSES];

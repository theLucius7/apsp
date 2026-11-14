//! The costs domain's advisory lock keys (the rules are in `apsp_panel_host::locks`).

use apsp_panel_host::locks::Key;

/// Changing expenses: recurring services, payments, labels, settings, imports and reminder
/// delivery all take this lock first.
pub const EXPENSES: Key = Key::new(73201067, "expenses");
/// Checking cloud resources' bills against their budgets and recorded expiries, one pass at
/// a time.
pub const CLOUD_RESOURCE_CHECKS: Key = Key::new(73201068, "cloud resource budget checks");

/// Every key above, for the application's check of all declarations.
pub const KEYS: &[Key] = &[EXPENSES, CLOUD_RESOURCE_CHECKS];

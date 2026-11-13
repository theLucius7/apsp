//! Governance's advisory lock keys (the rules are in `apsp_panel_host::locks`).

use apsp_panel_host::locks::Key;

/// Creating and changing administrators, so that login names stay unique.
pub const ADMINISTRATORS: Key = Key::new(73002001, "administrators");
/// Saving one administrator's workspace preferences: one key per administrator, the class
/// 73002000 with the administrator's id as the object ([`Key::offset`]). Two-integer keys are a
/// key space of their own, so no id reaches another purpose's key (until 2026-11-13 this was the
/// single key 73002000 plus the id: administrator 1 took the lock of [`ADMINISTRATORS`]).
pub const PREFERENCES: Key = Key::pair(73002000, 0, "administrator preferences");
/// Refreshing the tool advisory observations, one refresh at a time.
pub const TOOL_ADVISORIES: Key = Key::new(73002112, "tool advisory refresh");
/// Starting Passkey ceremonies: their capacity check counts the ceremonies of all accounts.
pub const PASSKEY_CEREMONIES: Key = Key::new(739104859, "Passkey ceremonies");

/// Every key above, for the application's check of all declarations.
pub const KEYS: &[Key] = &[
    ADMINISTRATORS,
    PREFERENCES,
    TOOL_ADVISORIES,
    PASSKEY_CEREMONIES,
];

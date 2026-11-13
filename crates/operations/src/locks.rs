//! The operations domain's advisory lock keys (the rules are in `apsp_panel_host::locks`).

use apsp_panel_host::locks::Key;

/// Backup records: registering, reporting on and retiring them, and retention.
pub const BACKUP_RECORDS: Key = Key::new(530018, "backup records");
/// A full panel backup: one runs at a time, and a queued backup step waits while it runs.
pub const FULL_BACKUP: Key = Key::new(530118, "full panel backup");

/// Every key above, for the application's check of all declarations.
pub const KEYS: &[Key] = &[BACKUP_RECORDS, FULL_BACKUP];

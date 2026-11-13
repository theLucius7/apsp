//! The resources domain's advisory lock keys (the rules are in `apsp_panel_host::locks`).

use apsp_panel_host::locks::{Key, TextKey};

/// One writer per cloud account ([`crate::cloud::lock`]); the text is the account id.
pub const CLOUD_ACCOUNT: TextKey = TextKey::new(739104830, "cloud account");
/// Adding Alibaba Cloud accounts, within the account limit.
pub const ALICLOUD_ACCOUNTS: Key = Key::new(739104831, "Alibaba Cloud account creation");
/// Adding Alibaba Cloud resources, within the resource limit.
pub const ALICLOUD_RESOURCES: Key = Key::new(739104832, "Alibaba Cloud resource creation");
/// Adding Hetzner accounts, within the account limit.
pub const HETZNER_ACCOUNTS: Key = Key::new(530119, "Hetzner account creation");
/// Recording observations of the linked cloud resources, one pass at a time.
pub const CLOUD_OBSERVATIONS: Key = Key::new(530019, "cloud resource observation");

/// Every key above except the text keys, for the application's check of all declarations.
pub const KEYS: &[Key] = &[
    ALICLOUD_ACCOUNTS,
    ALICLOUD_RESOURCES,
    HETZNER_ACCOUNTS,
    CLOUD_OBSERVATIONS,
];

/// Every text key above.
pub const TEXT_KEYS: &[TextKey] = &[CLOUD_ACCOUNT];

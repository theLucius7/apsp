//! The device channel with the application's interfaces connected. Kept at its
//! previous public path; the device configuration handlers now belong to traffic.

pub use apsp_panel_host::agent_api::*;
pub use apsp_panel_host::device_config::{bundle, manifest, record_apply_result};

/// Handles one device message once the domains' interfaces are connected.
pub async fn process_message(
    state: &crate::AppState,
    server_id: i64,
    message: apsp_protocol::Message,
) -> anyhow::Result<()> {
    crate::plugins::install();
    apsp_panel_host::agent_api::process_message(state, server_id, message).await
}

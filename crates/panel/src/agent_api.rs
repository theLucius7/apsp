//! The device channel with the application's interfaces connected.

pub use apsp_panel_host::agent_api::*;

/// Handles one device message once the domains' interfaces are connected.
pub async fn process_message(
    state: &apsp_panel_host::AppState,
    server_id: i64,
    message: apsp_protocol::Message,
) -> anyhow::Result<()> {
    crate::domains::install();
    apsp_panel_host::agent_api::process_message(state, server_id, message).await
}

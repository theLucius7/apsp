mod commands;
mod probes;
mod state;

use crate::{SharedState, artifacts::PanelClient};
use anyhow::Result;
use apsp_adapter_sdk::Privileged;
use std::sync::Arc;
use tokio::sync::watch;

pub async fn run(
    server_id: i64,
    allow_remote_commands: bool,
    state: SharedState,
    ops: Arc<dyn Privileged>,
    clients: watch::Receiver<Option<Arc<PanelClient>>>,
    retirement: Arc<crate::retirement::Retirement>,
) -> Result<()> {
    tokio::try_join!(
        commands::run(
            allow_remote_commands,
            state.clone(),
            ops.clone(),
            clients.clone(),
            retirement.clone()
        ),
        probes::run(server_id, state, ops, clients, retirement)
    )?;
    Ok(())
}

// Retirement performs local recovery even if new remote commands are disabled.
pub(crate) async fn cleanup_commands_for_retirement(
    state: &SharedState,
    ops: &dyn Privileged,
) -> Result<()> {
    commands::cleanup_for_retirement(state, ops).await
}

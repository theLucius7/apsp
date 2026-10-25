//! Bounded, volatile runtime logs for supervisors without a journal namespace.
//!
//! The supervisor starts one unprivileged collector for each output stream.
//! Collectors outlive Agent reconnects, retain their backlog only in memory,
//! and expose it over private local sockets. No log payload is written to disk.
mod collector;
mod contract;
mod reader;

pub(crate) use contract::available;
pub(crate) use reader::{follow, recent};

use anyhow::Result;

const LINE_LIMIT: usize = 16 * 1024;
const WIRE_LIMIT: usize = LINE_LIMIT * 6 + 128;
const RETENTION_SECONDS: i64 = 3600;
const PROTOCOL_LINE: &[u8] = b"APSP_RUNTIME_LOG_V1\n";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    since: i64,
    follow: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Line {
    at: i64,
    text: String,
}

/// Run the supervisor's nonprivileged, memory-only log receiver.
pub async fn collect(unit: &str, stream: &str) -> Result<()> {
    let path = contract::socket_path(unit, stream)?;
    let owner = contract::directory_owner(&path)?;
    let (local, _) = tokio::net::UnixStream::pair()?;
    anyhow::ensure!(
        owner != 0 && local.peer_cred()?.uid() == owner,
        "runtime log collector must use its protected nonprivileged account"
    );
    collector::serve(path, owner, tokio::io::stdin()).await
}

#[cfg(test)]
mod tests;

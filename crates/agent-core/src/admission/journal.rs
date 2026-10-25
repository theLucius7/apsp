//! Follows a runtime unit's volatile log namespace.
//!
//! Session lines name client and destination addresses, so the runtime unit
//! logs into a dedicated journald namespace configured for volatile, bounded
//! storage. Only message text is read; the caller keeps addresses and
//! accounting names and discards everything else.
use anyhow::{Context, Result};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::mpsc,
    time::timeout,
};

use super::RUNTIME_LOG_NAMESPACE;
const LINE_LIMIT: usize = 16 * 1024;

/// Whether the unit logs into the volatile runtime namespace.
pub(crate) async fn available(unit: &str) -> bool {
    let query = Command::new("systemctl")
        .args(["show", unit, "--property=LogNamespace", "--value"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    match timeout(Duration::from_secs(5), query).await {
        Ok(Ok(output)) => {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).trim() == RUNTIME_LOG_NAMESPACE
        }
        _ => false,
    }
}

/// Sends message lines logged since `since` (a journalctl time) until the
/// receiver closes or journalctl exits.
pub(crate) async fn follow(unit: &str, since: &str, lines: mpsc::Sender<String>) -> Result<()> {
    let mut child = Command::new("journalctl")
        .args([
            format!("--namespace={RUNTIME_LOG_NAMESPACE}"),
            format!("--unit={unit}"),
            format!("--since={since}"),
            "--follow".into(),
            "--output=cat".into(),
            "--no-pager".into(),
            "--quiet".into(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start runtime log reader")?;
    let mut reader = BufReader::new(child.stdout.take().context("runtime log has no output")?);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        if line.len() > LINE_LIMIT {
            continue;
        }
        if lines.send(line.trim_end().to_owned()).await.is_err() {
            break;
        }
    }
    let _ = child.kill().await;
    Ok(())
}

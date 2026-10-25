use super::{Line, Request, WIRE_LIMIT, contract};
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::{ServiceLogLine, ServiceLogs};
use apsp_protocol::now_timestamp;
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    sync::mpsc,
    task::JoinSet,
    time::timeout,
};

async fn connect(
    unit: &str,
    stream: &str,
    since: i64,
    follow: bool,
) -> Result<BufReader<UnixStream>> {
    let path = contract::socket_path(unit, stream)?;
    let owner = contract::socket_owner(&path)?;
    let mut socket = timeout(Duration::from_secs(2), UnixStream::connect(path)).await??;
    ensure!(
        socket.peer_cred()?.uid() == owner,
        "runtime log collector identity changed"
    );
    let mut request = serde_json::to_vec(&Request { since, follow })?;
    request.push(b'\n');
    timeout(Duration::from_secs(2), socket.write_all(&request)).await??;
    let mut reader = BufReader::new(socket);
    let mut greeting = Vec::new();
    timeout(
        Duration::from_secs(2),
        (&mut reader).take(64).read_until(b'\n', &mut greeting),
    )
    .await??;
    ensure!(
        greeting == super::PROTOCOL_LINE,
        "runtime log collector did not accept the reader"
    );
    Ok(reader)
}

async fn next(reader: &mut BufReader<UnixStream>) -> Result<Option<Line>> {
    let mut wire = Vec::new();
    let mut limited = reader.take((WIRE_LIMIT + 1) as u64);
    let count = limited.read_until(b'\n', &mut wire).await?;
    if count == 0 {
        return Ok(None);
    }
    ensure!(
        wire.len() <= WIRE_LIMIT && wire.last() == Some(&b'\n'),
        "runtime log frame exceeds limit"
    );
    let line: Line = serde_json::from_slice(&wire)?;
    ensure!(
        line.text.len() <= super::LINE_LIMIT,
        "runtime log line exceeds limit"
    );
    Ok(Some(line))
}

pub(crate) async fn follow(unit: &str, since: &str, sender: mpsc::Sender<String>) -> Result<()> {
    let age = match since {
        "-10min" => 600,
        "-2min" => 120,
        _ => anyhow::bail!("invalid runtime log range"),
    };
    let mut readers = JoinSet::new();
    for stream in ["stdout", "stderr"] {
        let (unit, sender) = (unit.to_owned(), sender.clone());
        readers.spawn(async move {
            let mut reader =
                connect(&unit, stream, now_timestamp().saturating_sub(age), true).await?;
            while let Some(line) = next(&mut reader).await? {
                if sender.send(line.text).await.is_err() {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        });
    }
    let result = readers
        .join_next()
        .await
        .context("runtime log reader missing")?;
    readers.abort_all();
    result?
}

pub(crate) async fn recent(unit: &str) -> Result<ServiceLogs> {
    let mut logs = ServiceLogs {
        lines: Vec::new(),
        truncated: false,
        service_events: false,
    };
    for stream in ["stdout", "stderr"] {
        let mut reader = connect(unit, stream, now_timestamp().saturating_sub(3600), false).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        let mut scanned = 0;
        let mut retained = std::collections::VecDeque::new();
        while let Some(line) = tokio::time::timeout_at(deadline, next(&mut reader)).await?? {
            scanned += line.text.len();
            if scanned > 4 * 1024 * 1024 {
                logs.truncated = true;
                break;
            }
            if crate::admission::session_level(&line.text) {
                continue;
            }
            retained.push_front(ServiceLogLine {
                text: line.text,
                priority: None,
                timestamp: Some(line.at),
            });
            if retained.len() == 100 {
                logs.truncated = true;
                break;
            }
        }
        logs.lines.extend(retained);
    }
    logs.lines.sort_by_key(|line| line.timestamp);
    if logs.lines.len() > 100 {
        logs.lines.drain(..logs.lines.len() - 100);
        logs.truncated = true;
    }
    Ok(logs)
}

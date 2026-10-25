use super::{LINE_LIMIT, Line, RETENTION_SECONDS, Request};
use anyhow::{Context, Result, ensure};
use apsp_protocol::now_timestamp;
use std::{
    collections::VecDeque,
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{Semaphore, broadcast},
    task::JoinSet,
    time::timeout,
};

const CAPACITY: usize = 8 * 1024 * 1024;
const CLIENTS: usize = 2;

#[derive(Clone)]
struct Event {
    at: i64,
    created: Instant,
    wire: Arc<[u8]>,
}

pub(super) struct Ring {
    events: VecDeque<Event>,
    bytes: usize,
    limit: usize,
    sender: broadcast::Sender<Event>,
}

impl Ring {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            events: VecDeque::new(),
            bytes: 0,
            limit,
            sender: broadcast::channel(64).0,
        }
    }

    fn prune(&mut self, now: Instant) {
        while self.events.front().is_some_and(|event| {
            now.saturating_duration_since(event.created).as_secs() >= RETENTION_SECONDS as u64
                || self.bytes > self.limit
        }) {
            let old = self.events.pop_front().expect("front event exists");
            self.bytes -= old.wire.len() + std::mem::size_of::<Event>();
        }
    }

    pub(super) fn push(&mut self, text: String, now: Instant) -> Result<()> {
        let at = now_timestamp();
        let mut wire = serde_json::to_vec(&Line { at, text })?;
        wire.push(b'\n');
        let event = Event {
            at,
            created: now,
            wire: wire.into(),
        };
        self.bytes += event.wire.len() + std::mem::size_of::<Event>();
        self.events.push_back(event.clone());
        self.prune(now);
        let _ = self.sender.send(event);
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct Splitter {
    pending: Vec<u8>,
    dropping: bool,
}

impl Splitter {
    pub(super) fn feed(
        &mut self,
        bytes: &[u8],
        mut line: impl FnMut(String) -> Result<()>,
    ) -> Result<()> {
        for byte in bytes {
            if *byte == b'\n' {
                if !self.dropping {
                    if self.pending.last() == Some(&b'\r') {
                        self.pending.pop();
                    }
                    if let Ok(text) = String::from_utf8(std::mem::take(&mut self.pending)) {
                        line(text)?;
                    }
                }
                self.pending.clear();
                self.dropping = false;
            } else if !self.dropping {
                if self.pending.len() == LINE_LIMIT {
                    self.pending.clear();
                    self.dropping = true;
                } else {
                    self.pending.push(*byte);
                }
            }
        }
        Ok(())
    }
}

struct SocketGuard {
    path: PathBuf,
    identity: (u64, u64),
}
impl Drop for SocketGuard {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
            (metadata.dev(), metadata.ino()) == self.identity && metadata.file_type().is_socket()
        }) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(super) async fn serve(
    path: PathBuf,
    owner: u32,
    mut input: impl AsyncRead + Unpin,
) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        ensure!(
            metadata.file_type().is_socket() && metadata.uid() == owner,
            "refusing to replace a foreign runtime log socket"
        );
        ensure!(
            UnixStream::connect(&path).await.is_err(),
            "runtime log collector is already active"
        );
        fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let metadata = fs::symlink_metadata(&path)?;
    let _guard = SocketGuard {
        path,
        identity: (metadata.dev(), metadata.ino()),
    };
    let ring = Arc::new(Mutex::new(Ring::new(CAPACITY)));
    let permits = Arc::new(Semaphore::new(CLIENTS));
    let mut peers = JoinSet::new();
    let mut bytes = [0; 4096];
    let mut splitter = Splitter::default();
    let mut expiry = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            count = input.read(&mut bytes) => {
                let count = count?;
                if count == 0 { break }
                let mut ring = ring.lock().map_err(|_| anyhow::anyhow!("runtime log buffer poisoned"))?;
                splitter.feed(&bytes[..count], |text| ring.push(text, Instant::now()))?;
            }
            peer = listener.accept() => {
                let (peer, _) = peer?;
                let Ok(permit) = permits.clone().try_acquire_owned() else { continue };
                let credential = peer.peer_cred()?;
                if credential.uid() != 0 && credential.uid() != owner { continue }
                let ring = ring.clone();
                peers.spawn(async move {
                    let _permit = permit;
                    let _ = stream(peer, ring).await;
                });
            }
            _ = expiry.tick() => {
                ring.lock().map_err(|_| anyhow::anyhow!("runtime log buffer poisoned"))?.prune(Instant::now());
            }
            _ = peers.join_next(), if !peers.is_empty() => {}
        }
    }
    peers.abort_all();
    Ok(())
}

async fn stream(mut peer: UnixStream, ring: Arc<Mutex<Ring>>) -> Result<()> {
    let mut request = Vec::new();
    let mut reader = BufReader::new((&mut peer).take(513));
    timeout(
        Duration::from_secs(2),
        reader.read_until(b'\n', &mut request),
    )
    .await??;
    ensure!(
        request.len() <= 512 && request.last() == Some(&b'\n'),
        "invalid runtime log request"
    );
    let request: Request = serde_json::from_slice(&request)?;
    let now = now_timestamp();
    ensure!(
        request.since >= 0 && request.since <= now + 60,
        "invalid runtime log time range"
    );
    let (mut history, mut receiver) = {
        let mut ring = ring
            .lock()
            .map_err(|_| anyhow::anyhow!("runtime log buffer poisoned"))?;
        ring.prune(Instant::now());
        let history = ring
            .events
            .iter()
            .filter(|event| event.at >= request.since.max(now - RETENTION_SECONDS))
            .cloned()
            .collect::<Vec<_>>();
        (history, ring.sender.subscribe())
    };
    if !request.follow {
        // A bounded administrative scan must start at the newest records.
        history.reverse();
    }
    timeout(Duration::from_secs(2), peer.write_all(super::PROTOCOL_LINE)).await??;
    timeout(Duration::from_secs(5), async {
        for event in history {
            peer.write_all(&event.wire).await?;
        }
        Ok::<(), std::io::Error>(())
    })
    .await
    .context("runtime log backlog reader is too slow")??;
    if request.follow {
        let mut unexpected = [0];
        loop {
            tokio::select! {
                count = peer.read(&mut unexpected) => {
                    ensure!(count? == 0, "runtime log reader sent another request");
                    return Ok(());
                }
                event = receiver.recv() => {
                    let event = event.context("runtime log reader lagged")?;
                    timeout(Duration::from_secs(2), peer.write_all(&event.wire)).await??;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backlog_evicts_by_bytes_and_expires_without_new_input() {
        let mut ring = Ring::new(1024);
        let now = Instant::now();
        for index in 0..100 {
            ring.push(format!("fixture-{index}: {}", "x".repeat(200)), now)
                .unwrap();
            assert!(ring.bytes <= 1024);
        }
        assert!(ring.events.len() < 5);
        let newest: Line = serde_json::from_slice(&ring.events.back().unwrap().wire).unwrap();
        assert!(newest.text.starts_with("fixture-99:"));
        ring.prune(now + Duration::from_secs(RETENTION_SECONDS as u64 + 1));
        assert!(ring.events.is_empty());
        assert_eq!(ring.bytes, 0);
    }

    #[test]
    fn oversized_unterminated_input_is_discarded_until_the_next_line() {
        let mut splitter = Splitter::default();
        let mut lines = Vec::new();
        for _ in 0..1000 {
            splitter
                .feed(&[b'x'; 4096], |line| {
                    lines.push(line);
                    Ok(())
                })
                .unwrap();
            assert!(splitter.pending.len() <= LINE_LIMIT);
        }
        splitter
            .feed(b"\nINFO recovered\r\n", |line| {
                lines.push(line);
                Ok(())
            })
            .unwrap();
        assert_eq!(lines, ["INFO recovered"]);
        assert!(splitter.pending.is_empty());
    }
}

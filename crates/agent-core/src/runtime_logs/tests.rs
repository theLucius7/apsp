use super::{Line, Request, collector, contract};
use std::{
    fs,
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    time::timeout,
};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("apsp-memory-logs-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn connect(path: &Path, follow: bool) -> BufReader<UnixStream> {
    let mut peer = timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(peer) = UnixStream::connect(path).await {
                break peer;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let mut request = serde_json::to_vec(&Request { since: 0, follow }).unwrap();
    request.push(b'\n');
    peer.write_all(&request).await.unwrap();
    let mut peer = BufReader::new(peer);
    let mut greeting = Vec::new();
    timeout(
        Duration::from_secs(1),
        peer.read_until(b'\n', &mut greeting),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(greeting, super::PROTOCOL_LINE);
    peer
}

async fn line(reader: &mut BufReader<UnixStream>) -> Option<String> {
    let mut wire = String::new();
    let count = timeout(Duration::from_secs(2), reader.read_line(&mut wire))
        .await
        .unwrap()
        .unwrap();
    (count != 0).then(|| serde_json::from_str::<Line>(&wire).unwrap().text)
}

#[tokio::test]
async fn reconnecting_readers_recover_memory_backlog_without_persisting_payloads() {
    let directory = Directory::new();
    let path = directory.0.join("stderr.sock");
    let (mut input, receiver) = tokio::io::duplex(4096);
    let (identity, _) = UnixStream::pair().unwrap();
    let owner = identity.peer_cred().unwrap().uid();
    let server = tokio::spawn(collector::serve(path.clone(), owner, receiver));
    let mut first = connect(&path, true).await;
    input.write_all(b"INFO fixture-connection\n").await.unwrap();
    assert_eq!(
        line(&mut first).await.as_deref(),
        Some("INFO fixture-connection")
    );
    drop(first);
    // Agent restarts do not terminate the independent collector. Idle reader
    // disconnects must also release their slots without requiring new logs.
    for _ in 0..5 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let mut peer = connect(&path, true).await;
        assert_eq!(
            line(&mut peer).await.as_deref(),
            Some("INFO fixture-connection")
        );
    }
    let entries = fs::read_dir(&directory.0)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].file_type().unwrap().is_socket());
    input.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn malformed_request_is_closed_without_stopping_the_collector() {
    let directory = Directory::new();
    let path = directory.0.join("stdout.sock");
    let (mut input, receiver) = tokio::io::duplex(4096);
    let (identity, _) = UnixStream::pair().unwrap();
    let server = tokio::spawn(collector::serve(
        path.clone(),
        identity.peer_cred().unwrap().uid(),
        receiver,
    ));
    let mut first = connect(&path, false).await;
    assert_eq!(line(&mut first).await, None);
    drop(first);
    let mut invalid = UnixStream::connect(&path).await.unwrap();
    invalid.write_all(&[b'x'; 513]).await.unwrap();
    let mut invalid = BufReader::new(invalid);
    assert_eq!(line(&mut invalid).await, None);
    input
        .write_all(b"WARN fixture-still-running\n")
        .await
        .unwrap();
    let mut good = connect(&path, true).await;
    assert_eq!(
        line(&mut good).await.as_deref(),
        Some("WARN fixture-still-running")
    );
    input.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn snapshots_scan_newest_first_while_followers_replay_in_event_order() {
    let directory = Directory::new();
    let path = directory.0.join("stdout.sock");
    let (mut input, receiver) = tokio::io::duplex(4096);
    let (identity, _) = UnixStream::pair().unwrap();
    let server = tokio::spawn(collector::serve(
        path.clone(),
        identity.peer_cred().unwrap().uid(),
        receiver,
    ));
    let mut live = connect(&path, true).await;
    input
        .write_all(b"WARN earlier\nWARN latest\n")
        .await
        .unwrap();
    assert_eq!(line(&mut live).await.as_deref(), Some("WARN earlier"));
    assert_eq!(line(&mut live).await.as_deref(), Some("WARN latest"));
    let mut snapshot = connect(&path, false).await;
    assert_eq!(line(&mut snapshot).await.as_deref(), Some("WARN latest"));
    assert_eq!(line(&mut snapshot).await.as_deref(), Some("WARN earlier"));
    assert_eq!(line(&mut snapshot).await, None);
    input.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn reader_limit_refuses_extra_peers_and_releases_disconnected_slots() {
    let directory = Directory::new();
    let path = directory.0.join("stdout.sock");
    let (mut input, receiver) = tokio::io::duplex(4096);
    let (identity, _) = UnixStream::pair().unwrap();
    let server = tokio::spawn(collector::serve(
        path.clone(),
        identity.peer_cred().unwrap().uid(),
        receiver,
    ));
    let first = connect(&path, true).await;
    let second = connect(&path, true).await;
    let mut extra = BufReader::new(UnixStream::connect(&path).await.unwrap());
    // The collector rejects this connection before accepting a request.
    assert_eq!(line(&mut extra).await, None);
    drop(first);
    tokio::time::sleep(Duration::from_millis(25)).await;
    let mut replacement = connect(&path, true).await;
    input.write_all(b"WARN reader-recovered\n").await.unwrap();
    assert_eq!(
        line(&mut replacement).await.as_deref(),
        Some("WARN reader-recovered")
    );
    drop(second);
    input.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn collector_refuses_existing_files_and_live_sockets() {
    let directory = Directory::new();
    let path = directory.0.join("stdout.sock");
    let (identity, _) = UnixStream::pair().unwrap();
    let owner = identity.peer_cred().unwrap().uid();
    fs::write(&path, "fixture-preserved").unwrap();
    assert!(
        collector::serve(path.clone(), owner, tokio::io::empty())
            .await
            .is_err()
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "fixture-preserved");
    fs::remove_file(&path).unwrap();
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    assert!(
        collector::serve(path.clone(), owner, tokio::io::empty())
            .await
            .is_err()
    );
    assert!(path.exists());
    drop(listener);
}

#[test]
fn unprotected_directories_are_never_accepted_for_runtime_logs() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = Directory::new();
    let path = directory.0.join("stdout.sock");
    fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(contract::directory_owner(&path).is_err());
    let linked = directory.0.join("linked");
    symlink(&directory.0, &linked).unwrap();
    assert!(contract::directory_owner(&linked.join("stdout.sock")).is_err());
}

#[test]
fn persistent_or_incomplete_service_contracts_do_not_advertise_admission() {
    let contract = concat!(
        "# APSP_RUNTIME_LOG_V1=memory-socket\n",
        "output_logger=\"/opt/apsp/core/current/apsp-agent runtime-log --unit ${RC_SVCNAME}.service --stream stdout\"\n",
        "error_logger=\"/opt/apsp/core/current/apsp-agent runtime-log --unit ${RC_SVCNAME}.service --stream stderr\"\n",
    );
    assert!(contract::has_contract(contract));
    assert!(!contract::has_contract(&format!(
        "{contract}output_log=/var/log/runtime.log\n"
    )));
    assert!(!contract::has_contract(
        &contract.replace("--stream stderr", "--stream stdout")
    ));
    for unit in [
        "../escape.service",
        "-option.service",
        "runtime",
        ".service",
    ] {
        assert!(contract::socket_path(unit, "stderr").is_err());
    }
    assert!(contract::socket_path("runtime@main.service", "../file").is_err());
}

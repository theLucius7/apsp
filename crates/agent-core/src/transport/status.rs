use super::{Runtime, status_snapshot::snapshot};
use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    time::timeout,
};

pub(super) struct BoundSocket {
    listener: UnixListener,
    path: PathBuf,
}

impl Drop for BoundSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(super) async fn bind(path: &Path) -> Result<BoundSocket> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    let parent = path.parent().context("status socket has no parent")?;
    tokio::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .await?;
    let parent_metadata = tokio::fs::symlink_metadata(parent).await?;
    anyhow::ensure!(
        parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink(),
        "status directory must be an ordinary directory"
    );
    anyhow::ensure!(
        parent_metadata.permissions().mode() & 0o022 == 0,
        "status directory must not be writable by group or others"
    );
    if let Ok(metadata) = tokio::fs::symlink_metadata(path).await {
        anyhow::ensure!(
            metadata.file_type().is_socket(),
            "status path already exists and is not a socket"
        );
        anyhow::ensure!(
            UnixStream::connect(path).await.is_err(),
            "agent status socket is already active"
        );
        tokio::fs::remove_file(path).await?;
    }
    let listener = UnixListener::bind(path)?;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    Ok(BoundSocket {
        listener,
        path: path.to_path_buf(),
    })
}

pub(super) async fn serve(bound: BoundSocket, runtime: Runtime) -> Result<()> {
    loop {
        let (mut socket, _) = bound.listener.accept().await?;
        let snapshot = snapshot(&runtime)?;
        let mut bytes = serde_json::to_vec(&snapshot)?;
        bytes.push(b'\n');
        if let Err(error) = timeout(Duration::from_secs(2), socket.write_all(&bytes)).await {
            tracing::debug!(%error, "status client timed out");
        }
    }
}

pub async fn status(path: &Path) -> Result<Value> {
    timeout(Duration::from_secs(5), async {
        let socket = UnixStream::connect(path)
            .await
            .context("connect to running agent status socket")?;
        let mut bytes = Vec::new();
        socket.take(65_537).read_to_end(&mut bytes).await?;
        anyhow::ensure!(bytes.len() <= 65_536, "status response exceeds size limit");
        Ok(serde_json::from_slice(&bytes)?)
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::State;
    use serde_json::json;
    use std::sync::{Arc, Mutex, atomic::AtomicBool};
    use tokio::task::JoinSet;

    #[tokio::test]
    async fn status_socket_is_private_reports_state_and_cleans_up_on_abort() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            std::path::PathBuf::from("/tmp").join(format!("sn-status-{}", uuid::Uuid::new_v4()));
        let socket = directory.join("status.sock");
        let runtime = Runtime {
            state: Arc::new(Mutex::new(State::open(Path::new(":memory:"))?)),
            modules: Arc::new(vec![]),
            capabilities: Arc::new(vec![]),
            connected: Arc::new(AtomicBool::new(true)),
            public_ips: Arc::new(vec![]),
            agent_version: "fixture-agent",
            retirement: None,
            cancellation: None,
            runtime_control: None,
            admission: None,
            telemetry: tokio::sync::watch::channel(Arc::new(
                crate::telemetry::cache::Snapshot::default(),
            ))
            .1,
        };
        let bound = bind(&socket).await?;
        assert_eq!(
            std::fs::metadata(&socket)?.permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&directory)?.permissions().mode() & 0o777,
            0o700
        );
        assert!(bind(&socket).await.is_err());
        let mut tasks = JoinSet::new();
        tasks.spawn(serve(bound, runtime.clone()));
        let value = status(&socket).await?;
        assert_eq!(value["agent_version"], "fixture-agent");
        assert_eq!(value["connected"], true);
        assert_eq!(value["applied"], json!({}));
        assert_eq!(value["pending_batches"], 0);
        assert_eq!(value["oversized_usage_batches"], 0);
        assert!(value["usage_outbox_error"].is_null());
        runtime.state.lock().unwrap().connection.execute(
            "INSERT INTO usage_outbox(epoch,seq,batch) VALUES (?1,'1',?2)",
            rusqlite::params![
                uuid::Uuid::nil().to_string(),
                "x".repeat(crate::usage::MAX_PENDING_USAGE_BYTES)
            ],
        )?;
        let blocked = status(&socket).await?;
        assert_eq!(blocked["pending_batches"], 1);
        assert_eq!(blocked["oversized_usage_batches"], 1);
        assert!(
            blocked["usage_outbox_error"]
                .as_str()
                .unwrap()
                .contains("ledger reconciliation")
        );
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        assert!(!socket.exists());
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[tokio::test]
    async fn existing_status_parent_mode_is_preserved_and_unsafe_locations_rejected() -> Result<()>
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory =
            PathBuf::from("/tmp").join(format!("sn-status-mode-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory)?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))?;
        let socket = directory.join("status.sock");
        let bound = bind(&socket).await?;
        assert_eq!(
            std::fs::metadata(&directory)?.permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::metadata(&socket)?.permissions().mode() & 0o777,
            0o600
        );
        assert!(bind(&socket).await.is_err());
        drop(bound);
        let alias = directory.join("alias");
        symlink(&directory, &alias)?;
        assert!(bind(&alias.join("status.sock")).await.is_err());
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o777))?;
        assert!(bind(&socket).await.is_err());
        assert_eq!(
            std::fs::metadata(&directory)?.permissions().mode() & 0o777,
            0o777
        );
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
}

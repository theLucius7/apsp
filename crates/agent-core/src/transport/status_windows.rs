use super::{Runtime, status_snapshot::snapshot};
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::Privileged;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions},
    time::timeout,
};

#[derive(Serialize, Deserialize)]
struct Endpoint {
    pipe: String,
    token: String,
}
pub(super) struct BoundSocket {
    listener: NamedPipeServer,
    path: PathBuf,
    endpoint: Endpoint,
}
impl Drop for BoundSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn pipe_name(path: &Path) -> String {
    format!(
        r"\\.\pipe\apsp-{:x}",
        Sha256::digest(path.to_string_lossy().to_lowercase().as_bytes())
    )
}

pub(super) async fn bind(path: &Path) -> Result<BoundSocket> {
    let pipe = pipe_name(path);
    let listener = ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(&pipe)
        .context("agent status pipe is already active or unavailable")?;
    let parent = path.parent().context("status path has no parent")?;
    let ops = crate::system::SystemOps;
    ops.create_dir(parent, 0o700, None).await?;
    let endpoint = Endpoint {
        pipe,
        token: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
    };
    ops.write_file(path, &serde_json::to_vec(&endpoint)?, 0o600, None)
        .await?;
    Ok(BoundSocket {
        listener,
        path: path.to_owned(),
        endpoint,
    })
}

pub(super) async fn serve(mut bound: BoundSocket, runtime: Runtime) -> Result<()> {
    loop {
        bound.listener.connect().await?;
        let next = ServerOptions::new()
            .reject_remote_clients(true)
            .create(&bound.endpoint.pipe)?;
        let mut socket = std::mem::replace(&mut bound.listener, next);
        let result = timeout(Duration::from_secs(2), async {
            let mut token = vec![0u8; bound.endpoint.token.len()];
            socket.read_exact(&mut token).await?;
            ensure!(
                token == bound.endpoint.token.as_bytes(),
                "invalid local status token"
            );
            let mut bytes = serde_json::to_vec(&snapshot(&runtime)?)?;
            bytes.push(b'\n');
            socket.write_all(&bytes).await?;
            Ok::<_, anyhow::Error>(())
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            tracing::debug!("local status client disconnected or timed out");
        }
    }
}

pub async fn status(path: &Path) -> Result<Value> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 4096,
        "invalid local status descriptor"
    );
    crate::system::check_private(path)?;
    let endpoint: Endpoint = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        endpoint.pipe == pipe_name(path) && endpoint.token.len() == 64,
        "invalid local status endpoint"
    );
    timeout(Duration::from_secs(5), async {
        let mut socket = loop {
            match ClientOptions::new().open(&endpoint.pipe) {
                Ok(socket) => break socket,
                Err(error) if error.raw_os_error() == Some(231) => {
                    tokio::time::sleep(Duration::from_millis(50)).await
                }
                Err(error) => return Err(error.into()),
            }
        };
        socket.write_all(endpoint.token.as_bytes()).await?;
        let mut bytes = Vec::new();
        socket.take(65_537).read_to_end(&mut bytes).await?;
        ensure!(bytes.len() <= 65_536, "status response exceeds size limit");
        Ok(serde_json::from_slice(&bytes)?)
    })
    .await?
}

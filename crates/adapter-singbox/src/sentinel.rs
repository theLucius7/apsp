use anyhow::{Context, Result, bail};
use std::{io::ErrorKind, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const PING: &[u8; 8] = b"apsp-HUP";
const SETTINGS: u8 = 4;
const PING_TYPE: u8 = 6;
const ACK: u8 = 1;

pub(crate) struct Sentinel(TcpStream);

impl Sentinel {
    pub(crate) async fn connect(address: &str) -> Result<Self> {
        let address = crate::native::stats_address(address)?;
        timeout(Duration::from_secs(3), async {
            let mut sentinel = Self(TcpStream::connect(address).await?);
            sentinel
                .0
                .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
                .await?;
            sentinel.write_frame(SETTINGS, 0, &[]).await?;
            sentinel.write_frame(PING_TYPE, 0, PING).await?;
            loop {
                let (kind, flags, payload) = sentinel.frame().await?;
                if kind == PING_TYPE && flags & ACK != 0 && payload == PING {
                    return Ok(sentinel);
                }
                sentinel.respond(kind, flags, &payload).await?;
            }
        })
        .await
        .context("statistics generation handshake timed out")?
    }

    pub(crate) async fn wait_closed(&mut self) -> Result<()> {
        timeout(Duration::from_secs(8), async {
            loop {
                match self.frame().await {
                    Ok((kind, flags, payload)) => self.respond(kind, flags, &payload).await?,
                    Err(error) => {
                        if error.downcast_ref::<std::io::Error>().is_some_and(|error| {
                            matches!(
                                error.kind(),
                                ErrorKind::UnexpectedEof
                                    | ErrorKind::ConnectionReset
                                    | ErrorKind::ConnectionAborted
                                    | ErrorKind::BrokenPipe
                            )
                        }) {
                            return Ok(());
                        }
                        return Err(error);
                    }
                }
            }
        })
        .await
        .context("previous runtime generation did not stop after reload")?
    }

    async fn frame(&mut self) -> Result<(u8, u8, Vec<u8>)> {
        let mut header = [0u8; 9];
        self.0.read_exact(&mut header).await?;
        let length = u32::from_be_bytes([0, header[0], header[1], header[2]]) as usize;
        // The default HTTP/2 receive frame limit is 16 KiB; we never negotiate a larger one.
        if length > 16_384 {
            bail!("oversized statistics HTTP/2 frame");
        }
        let mut payload = vec![0; length];
        self.0.read_exact(&mut payload).await?;
        Ok((header[3], header[4], payload))
    }

    async fn respond(&mut self, kind: u8, flags: u8, payload: &[u8]) -> Result<()> {
        if flags & ACK == 0 {
            match kind {
                SETTINGS => self.write_frame(SETTINGS, ACK, &[]).await?,
                PING_TYPE if payload.len() == 8 => {
                    self.write_frame(PING_TYPE, ACK, payload).await?
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn write_frame(&mut self, kind: u8, flags: u8, payload: &[u8]) -> Result<()> {
        let length = u32::try_from(payload.len())?.to_be_bytes();
        self.0
            .write_all(&[length[1], length[2], length[3], kind, flags, 0, 0, 0, 0])
            .await?;
        self.0.write_all(payload).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{net::TcpListener, sync::oneshot};

    #[tokio::test]
    async fn generation_barrier_requires_handshake_and_actual_connection_close() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (close, close_requested) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut peer = Sentinel(stream);
            let mut preface = [0; 24];
            peer.0.read_exact(&mut preface).await.unwrap();
            assert_eq!(&preface, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
            assert_eq!(peer.frame().await.unwrap(), (SETTINGS, 0, vec![]));
            assert_eq!(peer.frame().await.unwrap(), (PING_TYPE, 0, PING.to_vec()));
            peer.write_frame(SETTINGS, 0, &[]).await.unwrap();
            peer.write_frame(PING_TYPE, ACK, PING).await.unwrap();
            assert_eq!(peer.frame().await.unwrap(), (SETTINGS, ACK, vec![]));
            // A GOAWAY frame alone is not proof that the previous generation stopped.
            peer.write_frame(7, 0, &[0; 8]).await.unwrap();
            close_requested.await.unwrap();
        });
        let mut sentinel = Sentinel::connect(&address.to_string()).await.unwrap();
        let waiter = tokio::spawn(async move { sentinel.wait_closed().await });
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(!waiter.is_finished());
        close.send(()).unwrap();
        timeout(Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        server.await.unwrap();
    }
}

use crate::native::{Listener, Transport};
use anyhow::{Context, Result};
use apsp_adapter_sdk::Prepared;
use std::{io::ErrorKind, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::{TcpStream, UdpSocket},
    time::{Instant, timeout_at},
};

mod socket;
use tokio_rustls::{
    TlsConnector,
    rustls::{
        self,
        pki_types::{CertificateDer, ServerName, pem::PemObject},
    },
};

pub(crate) fn budget(target: &Prepared) -> Duration {
    let acme = target
        .spec
        .files
        .get("config.json")
        .and_then(|config| serde_json::from_str::<serde_json::Value>(config).ok())
        .is_some_and(|config| {
            config["certificate_providers"]
                .as_array()
                .is_some_and(|providers| {
                    providers.iter().any(|provider| provider["type"] == "acme")
                })
        });
    if acme {
        Duration::from_secs(240)
    } else {
        crate::HEALTH_TIMEOUT
    }
}

pub(crate) async fn probe(listener: &Listener) -> bool {
    probe_inner(listener, Instant::now() + Duration::from_secs(2))
        .await
        .is_ok()
}

async fn probe_inner(listener: &Listener, deadline: Instant) -> Result<()> {
    let Some(tls) = &listener.tls else {
        match listener.transport {
            Transport::Tcp => {
                timeout_at(deadline, TcpStream::connect(listener.address)).await??;
            }
            Transport::Udp => {
                // UDP has no connect handshake. Service state and statistics are checked separately.
                match timeout_at(deadline, UdpSocket::bind(listener.address)).await? {
                    Err(error) if error.kind() == ErrorKind::AddrInUse => {}
                    _ => anyhow::bail!("UDP listener is not bound"),
                }
            }
        }
        return Ok(());
    };
    let name = tls["server_name"].as_str().context("missing TLS name")?;
    let mut roots = rustls::RootCertStore::empty();
    if let Some(certificate) = tls.get("certificate") {
        let pem = if let Some(value) = certificate.as_str() {
            value.to_owned()
        } else {
            certificate
                .as_array()
                .context("invalid certificate chain")?
                .iter()
                .map(|line| line.as_str().context("invalid certificate line"))
                .collect::<Result<Vec<_>>>()?
                .join("\n")
        };
        for certificate in CertificateDer::pem_slice_iter(pem.as_bytes()) {
            roots.add(certificate?)?;
        }
        anyhow::ensure!(!roots.is_empty(), "empty certificate chain");
    } else {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = tls["alpn"]
        .as_array()
        .map(|protocols| {
            protocols
                .iter()
                .filter_map(|value| value.as_str().map(|value| value.as_bytes().to_vec()))
                .collect()
        })
        .unwrap_or_else(|| {
            if listener.transport == Transport::Udp {
                vec![b"h3".to_vec()]
            } else {
                vec![]
            }
        });
    match listener.transport {
        Transport::Tcp => {
            timeout_at(deadline, async {
                let socket = TcpStream::connect(listener.address).await?;
                TlsConnector::from(Arc::new(config))
                    .connect(ServerName::try_from(name.to_owned())?, socket)
                    .await?;
                Ok::<_, anyhow::Error>(())
            })
            .await??;
        }
        Transport::Udp => {
            let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(config)?;
            // Reserve part of the same two-second budget for confirmed driver
            // shutdown, including an unsuccessful or timed-out handshake.
            let handshake_deadline = deadline - Duration::from_millis(100);
            let bridge = match &listener.obfuscation {
                Some(password) => Some(
                    timeout_at(
                        handshake_deadline,
                        crate::obfuscation::Bridge::new(listener.address, password.clone()),
                    )
                    .await??,
                ),
                None => None,
            };
            let destination = bridge
                .as_ref()
                .map_or(listener.address, |bridge| bridge.address);
            let bind: SocketAddr = if destination.is_ipv6() {
                "[::]:0"
            } else {
                "0.0.0.0:0"
            }
            .parse()?;
            let socket = socket::Socket::bind(bind)?;
            let runtime = Arc::new(socket::Runtime::default());
            let mut endpoint = quinn::Endpoint::new_with_abstract_socket(
                Default::default(),
                None,
                socket.clone(),
                runtime.clone(),
            )?;
            endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(crypto)));
            let resources = socket::Resources {
                endpoint,
                socket,
                runtime,
            };
            let result = timeout_at(handshake_deadline, async {
                let connecting = resources.endpoint.connect(destination, name)?;
                let connection = if let Some(bridge) = &bridge {
                    tokio::select! {
                        result = connecting => result?,
                        result = bridge.forward() => {
                            result?;
                            anyhow::bail!("health bridge stopped before the handshake");
                        }
                    }
                } else {
                    connecting.await?
                };
                connection.close(0u32.into(), b"health check complete");
                Ok::<_, anyhow::Error>(())
            })
            .await
            .context("QUIC health handshake timed out")
            .and_then(|result| result);
            if let Some(bridge) = bridge {
                bridge.close();
            }
            timeout_at(deadline, resources.finish())
                .await
                .context("QUIC health cleanup timed out")??;
            result?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

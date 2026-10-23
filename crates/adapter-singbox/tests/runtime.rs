#![forbid(unsafe_code)]

mod support;

use anyhow::{Context, Result};
use apsp_adapter_sdk::{Adapter, Counter, Plan, Prepared, UsageSource};
use apsp_adapter_singbox::SingboxAdapter;
use std::{path::PathBuf, process::Stdio, time::Duration};
use support::{TempDir, TestOps, TestServices};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

async fn transfer(proxy_port: u16) -> Result<()> {
    let echo = TcpListener::bind("127.0.0.1:0").await?;
    let port = echo.local_addr()?.port();
    let server = tokio::spawn(async move {
        let (mut stream, _) = echo.accept().await?;
        let mut payload = vec![0; 1024];
        stream.read_exact(&mut payload).await?;
        stream.write_all(&payload).await?;
        Ok::<_, std::io::Error>(())
    });
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut client = TcpStream::connect(("127.0.0.1", proxy_port)).await?;
        // VLESS version, UUID, empty options, TCP command, destination port, IPv4 address.
        let mut request = vec![0];
        request.extend_from_slice(uuid::Uuid::from_u128(1).as_bytes());
        request.extend_from_slice(&[0, 1]);
        request.extend_from_slice(&port.to_be_bytes());
        request.extend_from_slice(&[1, 127, 0, 0, 1]);
        request.extend_from_slice(&[42; 1024]);
        client.write_all(&request).await?;
        let mut response = [0; 2];
        client.read_exact(&mut response).await?;
        anyhow::ensure!(response == [0, 0], "unexpected VLESS response");
        let mut echoed = [0; 1024];
        client.read_exact(&mut echoed).await?;
        anyhow::ensure!(echoed == [42; 1024], "echo payload differs");
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("local traffic timed out")?;
    server.abort();
    result
}

#[tokio::test]
#[ignore = "Requires exact upstream runtime built with with_v2ray_api; set APSP_TEST_SINGBOX and run --ignored"]
async fn real_check_stats_cumulative_reads_and_reload_generation() -> Result<()> {
    let binary =
        PathBuf::from(std::env::var("APSP_TEST_SINGBOX").context("set APSP_TEST_SINGBOX")?);
    let directory = TempDir::new();
    let stats_reservation = TcpListener::bind("127.0.0.1:0").await?;
    let stats_port = stats_reservation.local_addr()?.port();
    let inbound_reservation = TcpListener::bind("127.0.0.1:0").await?;
    let inbound_port = inbound_reservation.local_addr()?.port();
    // Plain local VLESS is a traffic fixture; production bundles are VLESS + Reality.
    let mut spec = directory.spec(
        stats_port,
        serde_json::json!([{
            "type":"vless", "tag":"test-node", "listen":"127.0.0.1", "listen_port":inbound_port,
            "users":[{"name":"u1_n3", "uuid":"00000000-0000-0000-0000-000000000001"}]
        }]),
    );
    spec.binary_path = binary.clone();
    let adapter = SingboxAdapter::new();
    let prepared = adapter
        .prepare(
            spec,
            &TestOps {
                version: None,
                ..TestOps::default()
            },
        )
        .await?;
    drop(stats_reservation);
    drop(inbound_reservation);
    let mut child = tokio::process::Command::new(&binary)
        .args(["run", "-c"])
        .arg(directory.0.join("config.json"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let services = TestServices {
        pid: child.id(),
        ..TestServices::default()
    };
    anyhow::ensure!(
        adapter.health(&prepared, &services).await?,
        "real runtime is unhealthy"
    );
    transfer(inbound_port).await?;
    let first = adapter.read_counters(&prepared).await?;
    anyhow::ensure!(
        first.len() == 1
            && first[0].stat_name == "u1_n3"
            && first[0].uplink >= 1024
            && first[0].downlink >= 1024,
        "real counters are missing: {first:?}"
    );
    let repeated = adapter.read_counters(&prepared).await?;
    anyhow::ensure!(first == repeated, "reading counters changed their values");
    adapter.apply(Plan::Reload, &prepared, &services).await?;
    anyhow::ensure!(
        adapter.health(&prepared, &services).await?,
        "reloaded runtime is unhealthy"
    );
    anyhow::ensure!(
        adapter.read_counters(&prepared).await?
            == vec![Counter {
                stat_name: "u1_n3".into(),
                uplink: 0,
                downlink: 0,
            }],
        "reload must expose configured zero counters before same-volume traffic"
    );
    transfer(inbound_port).await?;
    anyhow::ensure!(
        adapter.read_counters(&prepared).await? == first,
        "new generation did not start counters from zero"
    );
    child.kill().await?;
    child.wait().await?;
    Ok(())
}

#[tokio::test]
async fn failed_statistics_rpc_is_not_reported_as_zero_usage() -> Result<()> {
    let directory = TempDir::new();
    let reservation = TcpListener::bind("127.0.0.1:0").await?;
    let spec = directory.spec(reservation.local_addr()?.port(), serde_json::json!([]));
    let prepared = Prepared {
        spec,
        listen_ports: vec![],
    };
    drop(reservation);
    anyhow::ensure!(
        SingboxAdapter::new()
            .read_counters(&prepared)
            .await
            .is_err(),
        "failed statistics RPC was converted into zero counters"
    );
    Ok(())
}

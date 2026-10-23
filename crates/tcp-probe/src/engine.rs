use crate::{
    Journal, Options, Report, SOURCE_COMMIT, VERSION,
    model::{Engine, Parameters, Sample, TargetResult, now_millis, unicast},
};
use anyhow::{Result, ensure};
use std::{
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::{TcpStream, lookup_host},
    sync::{Semaphore, mpsc},
    task::JoinSet,
    time::{Instant, sleep, timeout_at},
};

pub(crate) type NetworkFuture<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;
pub(crate) trait Network: Send + Sync {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> NetworkFuture<'a, Vec<SocketAddr>>;
    fn connect(&self, address: SocketAddr) -> NetworkFuture<'_, ()>;
}

struct NativeNetwork;
impl Network for NativeNetwork {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> NetworkFuture<'a, Vec<SocketAddr>> {
        Box::pin(async move { Ok(resolved_addresses(lookup_host((host, port)).await?)) })
    }
    fn connect(&self, address: SocketAddr) -> NetworkFuture<'_, ()> {
        Box::pin(async move {
            let stream = TcpStream::connect(address).await?;
            // No application data is sent or read; dropping closes this connection.
            drop(stream);
            Ok(())
        })
    }
}

pub(crate) fn resolved_addresses(
    resolved: impl IntoIterator<Item = SocketAddr>,
) -> Vec<SocketAddr> {
    // Bound retained addresses after family selection so the resolver's ordering
    // cannot hide the first usable address of the requested family.
    let mut addresses = Vec::with_capacity(2);
    for address in resolved {
        if unicast(address.ip())
            && !addresses
                .iter()
                .any(|first: &SocketAddr| first.is_ipv4() == address.is_ipv4())
        {
            addresses.push(address);
            if addresses.len() == 2 {
                break;
            }
        }
    }
    addresses
}

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub dns: Duration,
    pub connect: Duration,
    pub interval: Duration,
    pub total: Duration,
    pub publication: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            dns: Duration::from_secs(2),
            connect: Duration::from_secs(1),
            interval: Duration::from_millis(250),
            total: Duration::from_secs(60),
            publication: Duration::from_secs(2),
        }
    }
}

pub async fn run(options: &Options, journal: &mut Journal) -> Result<Report> {
    run_with(options, journal, Arc::new(NativeNetwork), Limits::default()).await
}

pub async fn run_until(
    options: &Options,
    journal: &mut Journal,
    deadline: Instant,
) -> Result<Report> {
    run_with_deadline(
        options,
        journal,
        Arc::new(NativeNetwork),
        Limits::default(),
        Some(deadline),
    )
    .await
}

pub(crate) async fn run_with(
    options: &Options,
    journal: &mut Journal,
    network: Arc<dyn Network>,
    limits: Limits,
) -> Result<Report> {
    run_with_deadline(options, journal, network, limits, None).await
}

pub(crate) async fn run_with_deadline(
    options: &Options,
    journal: &mut Journal,
    network: Arc<dyn Network>,
    limits: Limits,
    outer_deadline: Option<Instant>,
) -> Result<Report> {
    let snapshot = journal.snapshot(options)?;
    ensure!(
        snapshot.valid() && matches!(options.count, 4 | 8) && matches!(options.concurrency, 1 | 2),
        "invalid probe parameters"
    );
    ensure!(
        SOURCE_COMMIT.is_none_or(
            |value| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        ),
        "invalid embedded source commit"
    );
    let started = Instant::now();
    // Caller inspection and output share the same absolute process budget.
    let deadline = outer_deadline.map_or(started + limits.total, |limit| {
        limit.min(started + limits.total)
    });
    ensure!(deadline > started, "execution deadline has elapsed");
    // Reserve bounded final publication time within the same total budget.
    let probe_deadline = deadline - limits.publication;
    #[cfg(test)]
    journal.record_probe_deadline(probe_deadline);
    let mut report = Report {
        schema: 1, method: "tcp_connect".into(),
        semantics: "连接成功率和 TCP 建连耗时；不是包丢失率、吞吐测速或上游 TcpQuality 兼容评分。地区与运营商是管理员配置标签，未指定地区不推断。".into(),
        engine: Engine { name: "apsp-native-tcp-connect-v1".into(), version: VERSION.into(), source_commit: SOURCE_COMMIT.map(str::to_owned) },
        started_at_ms: now_millis()?, finished_at_ms: None,
        parameters: Parameters { ip_version: options.ip_version, count: options.count, concurrency: options.concurrency, dns_timeout_ms: limits.dns.as_millis() as u64, connect_timeout_ms: limits.connect.as_millis() as u64, interval_ms: limits.interval.as_millis() as u64, total_timeout_ms: limits.total.as_millis() as u64 },
        target_digest: options.target_digest.clone(), targets: snapshot.targets.into_iter().map(TargetResult::queued).collect(),
        complete: false, deadline_exceeded: false, upload_enabled: false, ranking_enabled: false, speedtest_enabled: false,
    };
    journal.update(&report, None, deadline).await?;
    for index in 0..report.targets.len() {
        journal.update(&report, Some(index), deadline).await?;
    }
    let permits = Arc::new(Semaphore::new(options.concurrency.into()));
    let (updates, mut receiver) = mpsc::channel(16);
    let mut tasks = JoinSet::new();
    for (index, target) in report.targets.iter().cloned().enumerate() {
        let permits = permits.clone();
        let updates = updates.clone();
        let network = network.clone();
        let options = options.clone();
        tasks.spawn(async move {
            if let Ok(Ok(_permit)) = timeout_at(probe_deadline, permits.acquire_owned()).await
                && Instant::now() < probe_deadline
            {
                // Stop connections independently of any in-flight report I/O.
                let _ = timeout_at(
                    probe_deadline,
                    probe(
                        index,
                        target,
                        &options,
                        network.as_ref(),
                        limits,
                        &updates,
                        probe_deadline,
                    ),
                )
                .await;
            }
        });
    }
    drop(updates);
    while !report.targets.iter().all(|target| target.complete) && Instant::now() < probe_deadline {
        // A ready queued update must not consume the reserved publication time.
        // Finish an already-started atomic write within the unchanged total budget.
        let update = timeout_at(probe_deadline, receiver.recv()).await;
        let Ok(Some((index, target))) = update else {
            break;
        };
        report.targets[index] = target;
        journal.update(&report, Some(index), deadline).await?;
    }
    tasks.abort_all();
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            ensure!(error.is_cancelled(), "probe worker failed");
        }
    }
    // Workers have stopped and the bounded channel cannot receive new updates.
    // Retain attempts queued while report I/O crossed the probe cutoff.
    let mut buffered = vec![false; report.targets.len()];
    while let Ok((index, target)) = receiver.try_recv() {
        report.targets[index] = target;
        buffered[index] = true;
    }
    report.complete = report.targets.iter().all(|target| target.complete);
    report.deadline_exceeded = !report.complete && Instant::now() >= probe_deadline;
    for (index, buffered) in buffered.into_iter().enumerate() {
        if !report.targets[index].complete {
            let target = &mut report.targets[index];
            target.error = Some(
                if report.deadline_exceeded {
                    "total_timeout"
                } else {
                    "worker_failed"
                }
                .into(),
            );
            if target.status != "not_attempted" {
                target.status = "partial".into();
            }
        }
        if !report.targets[index].complete || buffered {
            journal.update(&report, Some(index), deadline).await?;
        }
    }
    report.finished_at_ms = Some(now_millis()?);
    journal.update(&report, None, deadline).await?;
    Ok(report)
}

async fn publish(
    index: usize,
    target: &TargetResult,
    updates: &mpsc::Sender<(usize, TargetResult)>,
) -> bool {
    updates.send((index, target.clone())).await.is_ok()
}

async fn probe(
    index: usize,
    mut target: TargetResult,
    options: &Options,
    network: &dyn Network,
    limits: Limits,
    updates: &mpsc::Sender<(usize, TargetResult)>,
    deadline: Instant,
) {
    target.status = "resolving".into();
    if !publish(index, &target, updates).await {
        return;
    }
    let address = if let Ok(ip) = target.target.target.parse::<IpAddr>() {
        options
            .ip_version
            .matches(ip)
            .then_some(SocketAddr::new(ip, target.target.port))
    } else {
        target.dns_attempts = 1;
        if !publish(index, &target, updates).await {
            return;
        }
        let started = Instant::now();
        if started >= deadline {
            return;
        }
        let dns_deadline = deadline.min(started + limits.dns);
        let outcome = timeout_at(
            dns_deadline,
            network.resolve(&target.target.target, target.target.port),
        )
        .await;
        // A ready future can be polled before timeout_at checks its elapsed timer.
        // Do not advance to a connection from a result received after the cutoff.
        if Instant::now() >= deadline {
            return;
        }
        match outcome {
            Ok(Ok(addresses)) if Instant::now() < dns_deadline => {
                addresses.into_iter().find(|address| {
                    options.ip_version.matches(address.ip())
                        && unicast(address.ip())
                        && address.port() == target.target.port
                })
            }
            result => {
                target.error = Some(
                    if result.is_err() || Instant::now() >= dns_deadline {
                        "dns_timeout"
                    } else {
                        "dns_error"
                    }
                    .into(),
                );
                target.status = "failed".into();
                target.complete = true;
                publish(index, &target, updates).await;
                return;
            }
        }
    };
    let Some(address) = address else {
        target.error = Some("ip_family_unavailable".into());
        target.status = "failed".into();
        target.complete = true;
        publish(index, &target, updates).await;
        return;
    };
    target.address = Some(address.to_string());
    target.status = "running".into();
    if !publish(index, &target, updates).await {
        return;
    }
    for iteration in 0..options.count {
        if iteration > 0 {
            sleep(limits.interval).await;
        }
        let Ok(attempted_at_ms) = now_millis() else {
            target.error = Some("utc_unavailable".into());
            target.status = "failed".into();
            publish(index, &target, updates).await;
            return;
        };
        let started = Instant::now();
        if started >= deadline {
            return;
        }
        let connect_deadline = deadline.min(started + limits.connect);
        let outcome = timeout_at(connect_deadline, network.connect(address)).await;
        let finished = Instant::now();
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let error = match outcome {
            _ if finished >= connect_deadline => Some("connect_timeout".into()),
            Ok(Ok(())) => None,
            Err(_) => Some("connect_timeout".into()),
            Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => {
                Some("connect_refused".into())
            }
            Ok(Err(_)) => Some("connect_error".into()),
        };
        target.samples.push(Sample {
            attempted_at_ms,
            elapsed_ms,
            latency_ms: error.is_none().then_some(elapsed_ms),
            error,
        });
        target.summarize();
        if finished >= deadline {
            publish(index, &target, updates).await;
            return;
        }
        if iteration + 1 == options.count {
            target.complete = true;
            target.status = "completed".into();
        }
        if !publish(index, &target, updates).await {
            return;
        }
    }
}

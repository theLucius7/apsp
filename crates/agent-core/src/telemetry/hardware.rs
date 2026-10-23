use anyhow::Result;
use apsp_adapter_sdk::Privileged;
use apsp_protocol::{DiskMetrics, GpuMetrics, Metrics};
use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub(super) struct Hardware {
    pub gpus: Vec<GpuMetrics>,
    pub connections: Option<(u64, u64)>,
    pub io: BTreeMap<String, DiskMetrics>,
}

impl Hardware {
    pub(super) fn apply(&self, metrics: &mut Metrics) {
        metrics.gpus = self.gpus.clone();
        if let Some((tcp, udp)) = self.connections {
            metrics.tcp_connections = Some(tcp);
            metrics.udp_connections = Some(udp);
        }
        for disk in &mut metrics.disks {
            let name = Path::new(&disk.name)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            if let Some(io) = self.io.get(name.as_ref()) {
                disk.read_bytes_per_sec = io.read_bytes_per_sec;
                disk.write_bytes_per_sec = io.write_bytes_per_sec;
                disk.read_iops = io.read_iops;
                disk.write_iops = io.write_iops;
                disk.await_ms = io.await_ms;
                disk.utilization_percent = io.utilization_percent;
            }
        }
    }
}

type IoCounters = BTreeMap<String, [u64; 7]>;

pub(super) fn disk_snapshot() -> IoCounters {
    let mut result = BTreeMap::new();
    if let Ok(text) = std::fs::read_to_string("/proc/diskstats") {
        for line in text.lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 14 {
                continue;
            }
            let counts = [3, 7, 5, 9, 6, 10, 12].map(|i| fields[i].parse::<u64>().ok());
            if let [
                Some(read),
                Some(write),
                Some(rx),
                Some(tx),
                Some(rtime),
                Some(wtime),
                Some(busy),
            ] = counts
            {
                result.insert(
                    fields[2].into(),
                    [
                        read,
                        write,
                        rx.saturating_mul(512),
                        tx.saturating_mul(512),
                        rtime,
                        wtime,
                        busy,
                    ],
                );
            }
        }
    }
    result
}

pub(super) fn disk_rates(
    previous: &IoCounters,
    current: &IoCounters,
    elapsed: f64,
) -> BTreeMap<String, DiskMetrics> {
    if elapsed <= 0.0 {
        return BTreeMap::new();
    }
    current
        .iter()
        .filter_map(|(name, current)| {
            let previous = previous.get(name)?;
            let delta: Option<Vec<_>> = current
                .iter()
                .zip(previous)
                .map(|(v, p)| v.checked_sub(*p))
                .collect();
            let delta = delta?;
            let ops = delta[0].saturating_add(delta[1]);
            Some((
                name.clone(),
                DiskMetrics {
                    name: name.clone(),
                    read_iops: Some(delta[0] as f64 / elapsed),
                    write_iops: Some(delta[1] as f64 / elapsed),
                    read_bytes_per_sec: Some(delta[2] as f64 / elapsed),
                    write_bytes_per_sec: Some(delta[3] as f64 / elapsed),
                    await_ms: Some(if ops > 0 {
                        delta[4].saturating_add(delta[5]) as f64 / ops as f64
                    } else {
                        0.0
                    }),
                    utilization_percent: Some((delta[6] as f64 / (elapsed * 10.0)).min(100.0)),
                    ..DiskMetrics::default()
                },
            ))
        })
        .collect()
}

fn finite(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn nvidia(text: &str) -> Vec<GpuMetrics> {
    text.lines()
        .take(32)
        .filter_map(|line| {
            let fields: Vec<_> = line.split(',').map(str::trim).collect();
            if fields.len() != 4 || fields[0].is_empty() {
                return None;
            }
            Some(GpuMetrics {
                model: fields[0].chars().take(256).collect(),
                usage_percent: finite(fields[1]).filter(|v| *v <= 100.0),
                memory_used: finite(fields[2]).map(|v| (v * 1024.0 * 1024.0) as u64),
                memory_total: finite(fields[3]).map(|v| (v * 1024.0 * 1024.0) as u64),
            })
        })
        .collect()
}

async fn command(ops: &dyn Privileged, program: &str, args: &[&str]) -> Option<String> {
    let args: Vec<_> = args.iter().map(|value| (*value).to_owned()).collect();
    tokio::time::timeout(
        Duration::from_secs(4),
        ops.execute_bounded(Path::new(program), &args, 4, 128 * 1024),
    )
    .await
    .ok()?
    .ok()
    .filter(|execution| !execution.timed_out && execution.output.success)
    .map(|execution| execution.output.stdout)
}

pub(super) async fn gpus(ops: &dyn Privileged) -> Vec<GpuMetrics> {
    if let Some(text) = command(
        ops,
        "nvidia-smi",
        &[
            "--query-gpu=name,utilization.gpu,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ],
    )
    .await
    {
        let result = nvidia(&text);
        if !result.is_empty() {
            return result;
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(text) = command(ops, "lspci", &["-mm"]).await {
            let models: Vec<_> = text
                .lines()
                .filter_map(|line| {
                    let fields: Vec<_> = line.split('"').collect();
                    if fields.len() < 6
                        || !["VGA", "3D", "Display"]
                            .iter()
                            .any(|class| fields[1].contains(class))
                    {
                        return None;
                    }
                    Some(GpuMetrics {
                        model: format!("{} {}", fields[3], fields[5])
                            .chars()
                            .take(256)
                            .collect(),
                        ..GpuMetrics::default()
                    })
                })
                .take(32)
                .collect();
            if !models.is_empty() {
                return models;
            }
        }
        let mut names = std::collections::BTreeSet::new();
        if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name
                    .strip_prefix("card")
                    .is_some_and(|v| !v.is_empty() && v.bytes().all(|v| v.is_ascii_digit()))
                {
                    continue;
                }
                if let Ok(value) = std::fs::read_to_string(entry.path().join("device/uevent")) {
                    let driver = value.lines().find_map(|line| line.strip_prefix("DRIVER="));
                    if let Some(driver) = driver {
                        names.insert(driver.to_owned());
                    }
                }
            }
        }
        names
            .into_iter()
            .take(32)
            .map(|model| GpuMetrics {
                model,
                ..GpuMetrics::default()
            })
            .collect()
    }
    #[cfg(target_os = "macos")]
    {
        let text = command(ops, "system_profiler", &["SPDisplaysDataType", "-json"])
            .await
            .unwrap_or_default();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        value["SPDisplaysDataType"]
            .as_array()
            .into_iter()
            .flatten()
            .take(32)
            .filter_map(|item| {
                Some(GpuMetrics {
                    model: item["sppci_model"].as_str()?.chars().take(256).collect(),
                    ..GpuMetrics::default()
                })
            })
            .collect()
    }
    #[cfg(target_os = "windows")]
    {
        let text = command(ops, "powershell.exe", &["-NoProfile", "-NonInteractive", "-Command", "@(Get-CimInstance Win32_VideoController | Select-Object Name,AdapterRAM) | ConvertTo-Json -Compress"]).await.unwrap_or_default();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let values = match value {
            serde_json::Value::Array(values) => values,
            value @ serde_json::Value::Object(_) => vec![value],
            _ => Vec::new(),
        };
        values
            .iter()
            .take(32)
            .filter_map(|item| {
                Some(GpuMetrics {
                    model: item["Name"].as_str()?.chars().take(256).collect(),
                    memory_total: item["AdapterRAM"].as_u64(),
                    ..GpuMetrics::default()
                })
            })
            .collect()
    }
    #[cfg(target_os = "freebsd")]
    {
        let text = command(ops, "pciconf", &["-lv"]).await.unwrap_or_default();
        let mut active = false;
        text.lines()
            .filter_map(|line| {
                if !line.starts_with(char::is_whitespace) {
                    active = line.contains("class=0x03");
                }
                if active {
                    line.trim()
                        .strip_prefix("device     = '")
                        .and_then(|v| v.strip_suffix('\''))
                } else {
                    None
                }
            })
            .take(32)
            .map(|model| GpuMetrics {
                model: model.chars().take(256).collect(),
                ..GpuMetrics::default()
            })
            .collect()
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "windows"
    )))]
    Vec::new()
}

pub(super) async fn refresh(
    ops: &dyn Privileged,
    previous: &mut (Instant, IoCounters),
) -> Result<Hardware> {
    let now = Instant::now();
    let current = disk_snapshot();
    let io = disk_rates(
        &previous.1,
        &current,
        now.duration_since(previous.0).as_secs_f64(),
    );
    *previous = (now, current);
    #[cfg(not(target_os = "linux"))]
    let connections = command(ops, "netstat", &["-an"])
        .await
        .map(|text| connection_counts(&text));
    #[cfg(target_os = "linux")]
    let connections = None;
    Ok(Hardware {
        gpus: gpus(ops).await,
        connections,
        io,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn io_reset_and_missing_gpu_metrics_are_not_reported_as_zero() {
        let previous = BTreeMap::from([("vda".into(), [10, 10, 100, 100, 20, 20, 100])]);
        let current = BTreeMap::from([("vda".into(), [14, 12, 1124, 1124, 28, 24, 200])]);
        let rates = disk_rates(&previous, &current, 2.0);
        assert_eq!(rates["vda"].read_iops, Some(2.0));
        assert_eq!(rates["vda"].read_bytes_per_sec, Some(512.0));
        assert!(disk_rates(&current, &previous, 2.0).is_empty());
        let gpu = nvidia("Fixture GPU, N/A, 1024, 4096\n");
        assert_eq!(gpu[0].usage_percent, None);
        assert_eq!(gpu[0].memory_used, Some(1024 * 1024 * 1024));
    }
}

#[cfg(any(not(target_os = "linux"), test))]
fn connection_counts(text: &str) -> (u64, u64) {
    text.lines().fold((0, 0), |(tcp, udp), line| {
        let protocol = line
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        (
            tcp + u64::from(matches!(protocol.as_str(), "tcp" | "tcp4" | "tcp6")),
            udp + u64::from(matches!(protocol.as_str(), "udp" | "udp4" | "udp6")),
        )
    })
}

#[test]
fn netstat_counts_protocol_rows_without_headers_or_unix_sockets() {
    assert_eq!(
        connection_counts(
            "Proto Local Address\nTCP 127.0.0.1:1\ntcp4 0 0 127.0.0.1.2\nUDP *:3\nudp6 *:4\nunix stream 0x1"
        ),
        (2, 2)
    );
}

use super::{ServiceBackend, SystemServiceManager};
use anyhow::{Result, ensure};
use apsp_adapter_sdk::{ServiceLogLine, ServiceLogs};
use std::{path::Path, time::Duration};

const MAX_BYTES: usize = 64 * 1024;
const MAX_LINES: usize = 100;

impl SystemServiceManager {
    pub(super) async fn read_recent_logs(&self, unit: &str) -> Result<ServiceLogs> {
        ensure!(
            !unit.is_empty()
                && unit.len() <= 255
                && !unit.starts_with('-')
                && unit.bytes().all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'.' | b'_' | b'-' | b'@')),
            "invalid service unit"
        );
        #[cfg(target_os = "linux")]
        if self.backend == ServiceBackend::OpenRc && crate::runtime_logs::available(unit) {
            return crate::runtime_logs::recent(unit).await;
        }
        // Runtime units log per-session lines into a volatile namespace; read
        // it, but show only warnings and errors as before.
        let namespace_query = [
            "show".to_owned(),
            unit.to_owned(),
            "--property=LogNamespace".to_owned(),
            "--value".to_owned(),
        ];
        let namespaced = self.backend == ServiceBackend::Systemd && {
            let query =
                self.privileged
                    .execute_bounded(Path::new("systemctl"), &namespace_query, 5, 1024);
            matches!(
                tokio::time::timeout(Duration::from_secs(6), query).await,
                Ok(Ok(output)) if output.output.success
                    && output.output.stdout.trim() == crate::admission::RUNTIME_LOG_NAMESPACE
            )
        };
        let (scan_lines, scan_bytes) = if namespaced {
            (5000, 4 * 1024 * 1024)
        } else {
            (MAX_LINES, MAX_BYTES)
        };
        let (program, args) = match self.backend {
            ServiceBackend::Systemd => {
                let mut args = vec![
                    "--no-pager".into(),
                    "--output=json".into(),
                    "--reverse".into(),
                    "--since=-1h".into(),
                    format!("--lines={scan_lines}"),
                    format!("--unit={unit}"),
                    "--output-fields=MESSAGE,PRIORITY,__REALTIME_TIMESTAMP".into(),
                ];
                if namespaced {
                    args.push(format!(
                        "--namespace={}",
                        crate::admission::RUNTIME_LOG_NAMESPACE
                    ));
                }
                ("journalctl", args)
            }
            ServiceBackend::Launchd => (
                "tail",
                vec![
                    "-c".into(),
                    MAX_BYTES.to_string(),
                    format!(
                        "/var/log/{}.log",
                        unit.strip_suffix(".service").unwrap_or(unit)
                    ),
                ],
            ),
            ServiceBackend::OpenRc => (
                "tail",
                vec![
                    "-c".into(),
                    MAX_BYTES.to_string(),
                    "/var/log/apsp/runtime.log".into(),
                ],
            ),
            ServiceBackend::FreeBsd => (
                "tail",
                vec![
                    "-c".into(),
                    MAX_BYTES.to_string(),
                    "/var/log/messages".into(),
                ],
            ),
            ServiceBackend::WindowsTask => {
                let name = unit.strip_suffix(".service").unwrap_or(unit);
                let script = format!(
                    "$ErrorActionPreference='Stop'; $query=\"*[System[TimeCreated[timediff(@SystemTime) <= 3600000]] and EventData[Data[@Name='TaskName']='\\{name}']]\"; Get-WinEvent -LogName 'Microsoft-Windows-TaskScheduler/Operational' -FilterXPath $query -MaxEvents {MAX_LINES} | ForEach-Object {{ @{{ MESSAGE=$_.Message; PRIORITY=([Math]::Min(7,[Math]::Max(0,$_.Level+1))).ToString(); __REALTIME_TIMESTAMP=(([DateTimeOffset]$_.TimeCreated).ToUnixTimeMilliseconds()*1000).ToString() }} | ConvertTo-Json -Compress }}"
                );
                (
                    "powershell.exe",
                    vec![
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-Command".into(),
                        script,
                    ],
                )
            }
            _ => anyhow::bail!("service log reading is not supported by this backend"),
        };
        let output = tokio::time::timeout(
            Duration::from_secs(12),
            self.privileged
                .execute_bounded(Path::new(program), &args, 10, scan_bytes),
        )
        .await??;
        ensure!(
            output.output.success && !output.timed_out,
            "service log read failed"
        );
        let mut logs = ServiceLogs {
            lines: Vec::new(),
            truncated: output.truncated,
            service_events: self.backend == ServiceBackend::WindowsTask,
        };
        if matches!(
            self.backend,
            ServiceBackend::Systemd | ServiceBackend::WindowsTask
        ) {
            for line in output.output.stdout.lines().take(scan_lines) {
                if logs.lines.len() == MAX_LINES {
                    break;
                }
                let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                    logs.truncated = true;
                    continue;
                };
                let Some(text) = value["MESSAGE"].as_str() else {
                    continue;
                };
                if namespaced && crate::admission::session_level(text) {
                    continue;
                }
                logs.lines.push(ServiceLogLine {
                    text: text.to_owned(),
                    priority: value["PRIORITY"]
                        .as_str()
                        .and_then(|value| value.parse().ok()),
                    timestamp: value["__REALTIME_TIMESTAMP"]
                        .as_str()
                        .and_then(|value| value.parse::<i64>().ok())
                        .map(|at| at / 1_000_000),
                });
            }
            logs.lines.reverse();
        } else {
            logs.lines = output
                .output
                .stdout
                .lines()
                .rev()
                .filter(|text| {
                    self.backend != ServiceBackend::FreeBsd || syslog_matches(text, unit)
                })
                .take(MAX_LINES)
                .map(|text| ServiceLogLine {
                    text: text.into(),
                    ..Default::default()
                })
                .collect();
            logs.lines.reverse();
        }
        logs.truncated |= logs.lines.len() == MAX_LINES || output.output.stdout.len() >= scan_bytes;
        Ok(logs)
    }
}

fn syslog_matches(text: &str, unit: &str) -> bool {
    let name = unit
        .strip_suffix(".service")
        .unwrap_or(unit)
        .replace(['@', '-', '.'], "_");
    let Some(tag) = text
        .split_whitespace()
        .nth(4)
        .and_then(|tag| tag.strip_suffix(':'))
    else {
        return false;
    };
    tag == name
        || tag
            .strip_prefix(&name)
            .and_then(|suffix| suffix.strip_prefix('['))
            .and_then(|suffix| suffix.strip_suffix(']'))
            .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()))
}

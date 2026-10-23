use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::{Execution, Privileged};
use std::{net::IpAddr, path::Path};

#[derive(Debug, PartialEq)]
pub(super) struct Measurement {
    pub received: u32,
    pub latency_ms: Option<f64>,
}

pub(super) async fn measure(address: IpAddr, ops: &dyn Privileged) -> Result<Measurement> {
    let (program, args) = command(std::env::consts::OS, address)?;
    let output = ops
        .execute_bounded(Path::new(program), &args, 8, 16 * 1024)
        .await
        .context("ICMP tool could not start; check installation and permissions")?;
    decode(&output, cfg!(windows))
}

fn command(platform: &str, address: IpAddr) -> Result<(&'static str, Vec<String>)> {
    if platform == "windows" {
        // Only a parsed numeric address is interpolated. JSON avoids localized ping output.
        let script = format!(
            r#"$ErrorActionPreference='Stop'; $p=New-Object System.Net.NetworkInformation.Ping; $times=@(); try {{ for($i=0;$i -lt 4;$i++) {{ $r=$p.Send([System.Net.IPAddress]::Parse('{address}'),1000); if($r.Status -eq [System.Net.NetworkInformation.IPStatus]::Success) {{ $times += $r.RoundtripTime }} elseif($r.Status -notin @([System.Net.NetworkInformation.IPStatus]::TimedOut,[System.Net.NetworkInformation.IPStatus]::DestinationHostUnreachable,[System.Net.NetworkInformation.IPStatus]::DestinationNetworkUnreachable,[System.Net.NetworkInformation.IPStatus]::DestinationUnreachable,[System.Net.NetworkInformation.IPStatus]::DestinationPortUnreachable,[System.Net.NetworkInformation.IPStatus]::TtlExpired,[System.Net.NetworkInformation.IPStatus]::TimeExceeded)) {{ throw 'ICMP measurement unavailable' }}; if($i -lt 3) {{ Start-Sleep -Milliseconds 200 }} }}; ConvertTo-Json -Compress -InputObject @{{sent=4;times=@($times)}} }} catch {{ [Console]::Error.WriteLine('ICMP measurement failed; check network and permissions'); exit 1 }} finally {{ $p.Dispose() }}"#
        );
        return Ok((
            "powershell.exe",
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                script,
            ],
        ));
    }
    let mut args: Vec<String> = ["LC_ALL=C", "LANG=C"].map(String::from).to_vec();
    let flags: &[&str] = match platform {
        // Quiet mode can hide local send failures while still counting them as transmitted.
        "linux" => &["ping", "-n", "-c", "4", "-W", "1", "-w", "6"],
        "freebsd" => &["ping", "-n", "-c", "4", "-W", "1000", "-t", "6"],
        // Apple's ping6 has no deadline flag; the bounded executor enforces it.
        "macos" if address.is_ipv6() => &["ping6", "-n", "-c", "4"],
        "macos" => &["ping", "-n", "-c", "4", "-W", "1000", "-t", "6"],
        _ => anyhow::bail!("ICMP is not supported on this platform"),
    };
    args.extend(flags.iter().map(|value| (*value).into()));
    args.push(address.to_string());
    Ok(("env", args))
}

fn decode(output: &Execution, windows: bool) -> Result<Measurement> {
    ensure!(!output.timed_out, "ICMP tool exceeded its deadline");
    ensure!(!output.truncated, "ICMP tool output was truncated");
    // A nonzero exit with a complete summary can represent genuine packet loss.
    let parsed = if windows {
        ensure!(output.output.success, "ICMP measurement unavailable");
        parse_windows(&output.output.stdout)
    } else {
        // A full packet summary does not prove that every send succeeded. BSD ping
        // also counts failed sends, and permission errors must remain unavailable.
        ensure!(
            output.output.stderr.trim().is_empty(),
            "ICMP measurement unavailable: {}",
            output
                .output
                .stderr
                .trim()
                .chars()
                .take(256)
                .collect::<String>()
        );
        parse_ping(&output.output.stdout)
    };
    parsed.with_context(|| {
        format!(
            "ICMP measurement unavailable: {}",
            output
                .output
                .stderr
                .trim()
                .chars()
                .take(256)
                .collect::<String>()
        )
    })
}

fn parse_windows(text: &str) -> Result<Measurement> {
    #[derive(serde::Deserialize)]
    struct Summary {
        sent: u32,
        times: Vec<f64>,
    }
    let summary: Summary = serde_json::from_str(text.trim())?;
    ensure!(
        summary.sent == 4
            && summary.times.len() <= 4
            && summary
                .times
                .iter()
                .all(|v| v.is_finite() && (0.0..=60_000.0).contains(v)),
        "invalid ICMP summary"
    );
    Ok(Measurement {
        received: summary.times.len() as u32,
        latency_ms: (!summary.times.is_empty())
            .then(|| summary.times.iter().sum::<f64>() / summary.times.len() as f64),
    })
}

fn parse_ping(text: &str) -> Result<Measurement> {
    let summary = text
        .lines()
        .find(|line| line.contains("packets transmitted") && line.contains("received"))
        .context("ICMP packet summary missing")?;
    let (sent, remaining) = summary
        .split_once("packets transmitted")
        .context("ICMP sent count missing")?;
    let sent: u32 = sent.trim().parse()?;
    let received: u32 = remaining
        .trim_start_matches(',')
        .split_whitespace()
        .next()
        .context("ICMP received count missing")?
        .parse()?;
    ensure!(
        sent == 4 && received <= sent,
        "ICMP sample is incomplete or invalid"
    );
    // Counts exclude duplicate responses and include late replies suppressed by BSD's -W.
    let latency_ms = if received == 0 {
        None
    } else {
        let average = text
            .lines()
            .filter(|line| line.contains("min/avg/max"))
            .find_map(|line| {
                line.split_once('=')
                    .and_then(|(_, value)| value.split('/').nth(1))
            })
            .context("ICMP timing summary missing")?;
        let average: f64 = average.trim().parse()?;
        ensure!(
            average.is_finite() && (0.0..=60_000.0).contains(&average),
            "invalid ICMP latency"
        );
        Some(average)
    };
    Ok(Measurement {
        received,
        latency_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use apsp_adapter_sdk::CommandOutput;

    #[test]
    fn summaries_cover_iputils_busybox_bsd_ipv6_duplicates_and_total_loss() -> Result<()> {
        for text in [
            "4 packets transmitted, 3 received, +1 duplicates, 25% packet loss, time 3000ms\nrtt min/avg/max/mdev = 0.010/12.500/30.0/1.0 ms",
            "4 packets transmitted, 3 packets received, 25% packet loss\nround-trip min/avg/max = 0.010/12.500/30.000 ms",
            "4 packets transmitted, 3 packets received, +1 duplicates, 25.0% packet loss\nround-trip min/avg/max/std-dev = 0.010/12.500/30.000/1.0 ms",
        ] {
            assert_eq!(
                parse_ping(text)?,
                Measurement {
                    received: 3,
                    latency_ms: Some(12.5)
                }
            );
        }
        let output = Execution {
            output: CommandOutput {
                success: false,
                stdout: "4 packets transmitted, 0 received, 100% packet loss".into(),
                stderr: String::new(),
            },
            ..Default::default()
        };
        assert_eq!(
            decode(&output, false)?,
            Measurement {
                received: 0,
                latency_ms: None
            }
        );
        assert!(
            decode(
                &Execution {
                    timed_out: true,
                    ..output.clone()
                },
                false
            )
            .is_err()
        );
        assert!(
            decode(
                &Execution {
                    truncated: true,
                    ..output
                },
                false
            )
            .is_err()
        );
        for text in [
            "",
            "100% packet loss",
            "ping: permission denied",
            "3 packets transmitted, 0 received",
            "4 packets transmitted, 5 received",
            "4 packets transmitted, 1 received",
            "4 packets transmitted, 1 received\nrtt min/avg/max = 1/NaN/2 ms",
        ] {
            assert!(parse_ping(text).is_err(), "{text}");
        }
        Ok(())
    }

    #[test]
    fn unix_diagnostics_reject_complete_summaries_even_when_exit_succeeds() {
        for success in [false, true] {
            for summary in [
                "4 packets transmitted, 0 received, 100% packet loss",
                "4 packets transmitted, 3 packets received, 25% packet loss\nround-trip min/avg/max = 0.010/12.500/30.000 ms",
            ] {
                for diagnostic in [
                    "ping: sendmsg: Operation not permitted\n",
                    "ping: sendto: Permission denied\n",
                    "ping: sendto: partial write\n",
                    "ping: warning: socket could not be configured\n",
                ] {
                    let output = Execution {
                        output: CommandOutput {
                            success,
                            stdout: summary.into(),
                            stderr: diagnostic.into(),
                        },
                        ..Default::default()
                    };
                    let error = decode(&output, false).unwrap_err().to_string();
                    assert!(error.contains("ICMP measurement unavailable"));
                    assert!(error.contains(diagnostic.trim()));
                }
            }
        }
        let output = Execution {
            output: CommandOutput {
                success: false,
                stdout: "4 packets transmitted, 0 received, 100% packet loss".into(),
                stderr: format!(
                    "ping: sendto: Permission denied {}UNBOUNDED_TAIL",
                    "x".repeat(512)
                ),
            },
            ..Default::default()
        };
        let error = decode(&output, false).unwrap_err().to_string();
        assert!(error.contains("Permission denied"));
        assert!(!error.contains("UNBOUNDED_TAIL"));
    }

    #[test]
    fn windows_json_preserves_zero_latency_and_rejects_partial_samples() -> Result<()> {
        assert_eq!(
            parse_windows(r#"{"sent":4,"times":[0,0,3]}"#)?,
            Measurement {
                received: 3,
                latency_ms: Some(1.0)
            }
        );
        assert_eq!(parse_windows(r#"{"sent":4,"times":[]}"#)?.received, 0);
        for text in [
            r#"{"sent":3,"times":[]}"#,
            r#"{"sent":4,"times":[-1]}"#,
            r#"{"sent":4,"times":[1,2,3,4,5]}"#,
            "本地化错误",
        ] {
            assert!(parse_windows(text).is_err());
        }
        Ok(())
    }

    #[test]
    fn platform_commands_are_numeric_localized_and_bounded() -> Result<()> {
        for platform in ["linux", "freebsd", "macos", "windows"] {
            for address in ["127.0.0.1", "::1"] {
                let (program, args) = command(platform, address.parse()?)?;
                if platform == "windows" {
                    assert_eq!(program, "powershell.exe");
                    assert!(
                        args.last()
                            .unwrap()
                            .contains(&format!("Parse('{address}')"))
                    );
                } else {
                    assert_eq!(program, "env");
                    assert_eq!(args[0], "LC_ALL=C");
                    assert_eq!(args.last().unwrap(), address);
                    assert!(!args.contains(&"-q".into()));
                    if platform == "macos" && address == "::1" {
                        assert_eq!(args[2], "ping6");
                        assert_eq!(&args[3..5], ["-n", "-c"]);
                        assert_eq!(args[5], "4");
                        // BSD -w/-W/-t select node information queries or a
                        // temporary source address, never a transfer deadline.
                        for unrelated in ["-X", "-w", "-W", "-t"] {
                            assert!(!args.contains(&unrelated.into()));
                        }
                    } else {
                        assert!(args.contains(&"6".into()));
                    }
                }
            }
        }
        Ok(())
    }
}

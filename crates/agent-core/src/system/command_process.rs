use super::{Command, Context, Duration, Path, Result, ensure, execution};
use apsp_adapter_sdk::{
    CommandObserver, CommandOutput, CommandProcessIdentity, ControlledExecution, Execution,
};
use std::process::Stdio;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    time::{Instant, timeout},
};
use uuid::Uuid;

struct Guard(u32);
impl Drop for Guard {
    fn drop(&mut self) {
        #[cfg(unix)]
        let mut signal = {
            let mut signal = std::process::Command::new("kill");
            signal.args(["-KILL", "--", &format!("-{}", self.0)]);
            signal
        };
        #[cfg(windows)]
        let mut signal = {
            let mut signal = std::process::Command::new("taskkill.exe");
            signal.args(["/PID", &self.0.to_string(), "/T", "/F"]);
            signal
        };
        let _ = signal.stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

async fn identity(pid: u32) -> Result<Option<String>> {
    #[cfg(target_os = "linux")]
    {
        let path = format!("/proc/{pid}/stat");
        let stat = match tokio::fs::read_to_string(path).await {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let fields: Vec<_> = stat
            .rsplit_once(')')
            .context("invalid process identity")?
            .1
            .split_whitespace()
            .collect();
        ensure!(fields.len() > 19, "incomplete process identity");
        let boot = tokio::fs::read_to_string("/proc/sys/kernel/random/boot_id").await?;
        Ok(Some(format!("{}:{}", boot.trim(), fields[19])))
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        let output = timeout(
            Duration::from_secs(3),
            Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "lstart="])
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        let value = String::from_utf8(output.stdout)?;
        Ok((!value.trim().is_empty()).then(|| value.trim().to_owned()))
    }
    #[cfg(windows)]
    {
        // Use .NET directly: cmdlet module autoloading can take many seconds on
        // a cold Windows ARM64 host. A process that exits before its start time
        // is read is "gone", not an unreadable identity; only an existing
        // process whose start time cannot be read is an error.
        let script = format!(
            "try {{ $p=[System.Diagnostics.Process]::GetProcessById({pid}) }} catch [System.ArgumentException] {{ exit 0 }}; try {{ $p.StartTime.ToUniversalTime().Ticks }} catch {{ $m=$_.Exception.Message; try {{ [void][System.Diagnostics.Process]::GetProcessById({pid}) }} catch [System.ArgumentException] {{ exit 0 }}; [Console]::Error.WriteLine($m); exit 2 }}; exit 0"
        );
        let output = timeout(
            Duration::from_secs(20),
            Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        ensure!(
            output.status.success(),
            "process identity inspection failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(300)
                .collect::<String>()
        );
        let value = String::from_utf8(output.stdout)?;
        Ok((!value.trim().is_empty()).then(|| value.trim().to_owned()))
    }
}

#[cfg(unix)]
async fn group_live(pid: u32) -> Result<bool> {
    let output = timeout(
        Duration::from_secs(3),
        Command::new("ps")
            // POSIX form: `-A` lists every process (BSD `-e` appends each
            // environment instead) and one `-o` per column, because BSD ps
            // treats everything after the first `=` as that column's header.
            .args(["-A", "-o", "pid=", "-o", "pgid=", "-o", "stat="])
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    ensure!(output.status.success(), "process group inspection failed");
    for line in String::from_utf8(output.stdout)?.lines() {
        let columns: Vec<_> = line.split_whitespace().collect();
        ensure!(columns.len() == 3, "invalid process group inspection");
        if columns[1].parse::<u32>()? == pid && !columns[2].starts_with('Z') {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn signal(pid: u32) -> Result<()> {
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("kill");
        command.args(["-KILL", "--", &format!("-{pid}")]);
        command
    };
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("taskkill.exe");
        command.args(["/PID", &pid.to_string(), "/T", "/F"]);
        command
    };
    // A concurrently exiting group may already be gone. Inspection below decides
    // whether cleanup succeeded, not the signal utility's exit code.
    timeout(
        Duration::from_secs(5),
        command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await??;
    Ok(())
}

pub(super) async fn recover(process: &CommandProcessIdentity) -> Result<()> {
    ensure!(
        process.pid > 1 && !process.started.is_empty(),
        "invalid command process identity"
    );
    #[cfg(target_os = "linux")]
    {
        let boot = tokio::fs::read_to_string("/proc/sys/kernel/random/boot_id").await?;
        if !process.started.starts_with(&format!("{}:", boot.trim())) {
            return Ok(());
        }
    }
    if identity(process.pid)
        .await?
        .is_some_and(|current| current != process.started)
    {
        // A new process owning this identifier means the original group no longer
        // owns it. Never signal the new process after PID reuse or a host reboot.
        return Ok(());
    }
    #[cfg(unix)]
    {
        if !group_live(process.pid).await? {
            return Ok(());
        }
        signal(process.pid).await?;
        timeout(Duration::from_secs(5), async {
            while group_live(process.pid).await? {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("command process group cleanup was not confirmed")??;
    }
    #[cfg(windows)]
    {
        if identity(process.pid).await?.is_none() {
            return Ok(());
        }
        signal(process.pid).await?;
        ensure!(
            identity(process.pid).await?.is_none(),
            "command process cleanup was not confirmed"
        );
    }
    Ok(())
}

fn gated_command(program: &Path, args: &[String], token: &str) -> Command {
    #[cfg(unix)]
    {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "IFS= read -r gate || exit 125; [ \"$gate\" = \"$1\" ] || exit 125; shift; printf 'ready\\n'; exec \"$@\"", "apsp-command", token]);
        command.arg(program).args(args).process_group(0);
        command
    }
    #[cfg(windows)]
    {
        let arguments = args
            .iter()
            .map(|arg| super::ps_quote(arg))
            .collect::<Vec<_>>()
            .join(" ");
        let script = format!(
            "if ([Console]::ReadLine() -cne '{}') {{ exit 125 }}; [Console]::Out.WriteLine('ready'); & {} {}; exit $LASTEXITCODE",
            token,
            super::ps_quote(&program.to_string_lossy()),
            arguments
        );
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        command
    }
}

pub(super) async fn execute(
    program: &Path,
    args: &[String],
    seconds: u32,
    maximum: usize,
    observer: &dyn CommandObserver,
) -> Result<ControlledExecution> {
    ensure!(
        (1..=600).contains(&seconds) && (1..=1024 * 1024).contains(&maximum),
        "invalid command limits"
    );
    let token = Uuid::new_v4().to_string();
    let mut child = gated_command(program, args, &token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("start gated command")?;
    let pid = child.id().context("command has no process identifier")?;
    let guard = Guard(pid);
    let process = CommandProcessIdentity {
        pid,
        started: identity(pid)
            .await?
            .context("command disappeared before registration")?,
    };
    // A failed journal write leaves the payload behind the stdin gate. Closing
    // stdin or killing this wrapper cannot execute the administrator's command.
    if let Err(error) = observer.spawned(&process) {
        recover(&process).await?;
        child.wait().await?;
        return Err(error);
    }
    let mut stdout = BufReader::new(child.stdout.take().context("command stdout is missing")?);
    let stderr = child.stderr.take().context("command stderr is missing")?;
    let cancelled_before_start = observer.cancellation_requested();
    if !cancelled_before_start {
        let start = async {
            let mut input = child
                .stdin
                .take()
                .context("command start gate is missing")?;
            input.write_all(format!("{token}\n").as_bytes()).await?;
            input.shutdown().await?;
            let mut ready = String::new();
            stdout.read_line(&mut ready).await?;
            ensure!(
                ready.trim() == "ready",
                "command start was not acknowledged"
            );
            observer.started()
        };
        if let Err(error) = timeout(Duration::from_secs(5), start)
            .await
            .context("command start gate timed out")
            .and_then(|r| r)
        {
            recover(&process).await?;
            child.wait().await?;
            return Err(error);
        }
    }
    let mut output = tokio::spawn(execution::drain(stdout, maximum));
    let mut errors = tokio::spawn(execution::drain(stderr, maximum));
    let deadline = Instant::now() + Duration::from_secs(u64::from(seconds));
    let observed: Result<(bool, bool, bool)> = async {
        loop {
            if let Some(status) = child.try_wait()? {
                break Ok((status.success(), false, false));
            }
            if observer.cancellation_requested() {
                break Ok((false, false, true));
            }
            if Instant::now() >= deadline {
                break Ok((false, true, false));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    .await;
    // Clean up background descendants even after a successful parent exit.
    let cleanup = recover(&process).await;
    let waited = timeout(Duration::from_secs(5), child.wait()).await;
    if let Err(error) = cleanup {
        output.abort();
        errors.abort();
        return Err(error);
    }
    waited.context("command exit was not confirmed")??;
    // The guard's unconditional KILL is needed on future cancellation only.
    std::mem::forget(guard);
    let collected = timeout(Duration::from_secs(5), async { (&mut output).await? }).await;
    let failed_output = collected.is_err() || collected.as_ref().is_ok_and(Result::is_err);
    if failed_output {
        output.abort();
        errors.abort();
    }
    let (out, cut_out) = collected.context("command stdout did not close")??;
    let collected = timeout(Duration::from_secs(5), &mut errors).await;
    if collected.is_err() {
        errors.abort();
    }
    let (err, cut_err) = collected.context("command stderr did not close")???;
    let (success, timed_out, cancelled) = observed?;
    ensure!(
        !cancelled || cfg!(unix),
        "running command cancellation is not confirmed on this platform"
    );
    Ok(ControlledExecution {
        execution: Execution {
            output: CommandOutput {
                success,
                stdout: out,
                stderr: err,
            },
            timed_out,
            truncated: cut_out || cut_err,
        },
        cancelled,
    })
}

#[cfg(all(test, unix))]
mod tests;

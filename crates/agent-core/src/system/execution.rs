use super::*;
use apsp_adapter_sdk::Execution;
use apsp_adapter_sdk::ManagedProcess;
use std::process::Stdio;
use tokio::{io::AsyncReadExt, time::timeout};

#[cfg(unix)]
fn managed_signal_target(id: u32) -> String {
    // launchd owns the service process group; its children must stay inside that group.
    if cfg!(target_os = "macos") {
        id.to_string()
    } else {
        format!("-{id}")
    }
}

struct ManagedChild {
    child: tokio::process::Child,
    id: u32,
    exit_code: Option<i32>,
}
impl Drop for ManagedChild {
    fn drop(&mut self) {
        if self.child.id().is_some() {
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("kill")
                    .args(["-KILL", "--", &managed_signal_target(self.id)])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            #[cfg(windows)]
            {
                let _ = std::process::Command::new("taskkill.exe")
                    .args(["/PID", &self.id.to_string(), "/T", "/F"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }
}
impl ManagedProcess for ManagedChild {
    fn id(&self) -> u32 {
        self.id
    }
    fn try_wait(&mut self) -> Result<Option<bool>> {
        Ok(self.child.try_wait()?.map(|status| {
            self.exit_code = status.code();
            status.success()
        }))
    }
    fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }
    fn terminate(&mut self) -> apsp_adapter_sdk::BoxFuture<'_, ()> {
        Box::pin(async move {
            #[cfg(unix)]
            {
                let _ = Command::new("kill")
                    .args(["-TERM", "--", &managed_signal_target(self.id)])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
            #[cfg(windows)]
            {
                let _ = Command::new("taskkill.exe")
                    .args(["/PID", &self.id.to_string(), "/T", "/F"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
            if timeout(Duration::from_secs(10), self.child.wait())
                .await
                .is_err()
            {
                #[cfg(unix)]
                {
                    let _ = Command::new("kill")
                        .args(["-KILL", "--", &managed_signal_target(self.id)])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .await;
                }
                self.child.kill().await?;
            }
            Ok(())
        })
    }
}

pub(super) fn spawn(program: &Path, args: &[String]) -> Result<Box<dyn ManagedProcess>> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(all(unix, not(target_os = "macos")))]
    command.process_group(0);
    let child = command.spawn()?;
    let id = child.id().context("managed process has no identifier")?;
    Ok(Box::new(ManagedChild {
        child,
        id,
        exit_code: None,
    }))
}

struct CommandGuard(u32);
impl Drop for CommandGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        let mut cleanup = {
            let mut command = std::process::Command::new("kill");
            command.args(["-KILL", "--", &format!("-{}", self.0)]);
            command
        };
        #[cfg(windows)]
        let mut cleanup = {
            let mut command = std::process::Command::new("taskkill.exe");
            command.args(["/PID", &self.0.to_string(), "/T", "/F"]);
            command
        };
        let _ = cleanup.stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

pub(super) async fn drain(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    maximum: usize,
) -> Result<(String, bool)> {
    let mut saved = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let length = reader.read(&mut buffer).await?;
        if length == 0 {
            break;
        }
        let keep = length.min(maximum.saturating_sub(saved.len()));
        saved.extend_from_slice(&buffer[..keep]);
        truncated |= keep != length;
    }
    // Normalize NUL like invalid UTF-8 for a displayable, persistable report,
    // then bound the resulting UTF-8 bytes.
    let mut text = String::from_utf8_lossy(&saved).replace('\0', "\u{fffd}");
    let mut limit = text.len().min(maximum);
    while !text.is_char_boundary(limit) {
        limit -= 1;
    }
    truncated |= text.len() > limit;
    text.truncate(limit);
    Ok((text, truncated))
}

pub(super) async fn execute(
    program: &Path,
    args: &[String],
    seconds: u32,
    maximum: usize,
) -> Result<Execution> {
    ensure!(
        (1..=3600).contains(&seconds) && (1..=1024 * 1024).contains(&maximum),
        "invalid command limits"
    );
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().context("start command")?;
    let id = child.id().context("command has no process identifier")?;
    let cleanup = CommandGuard(id);
    let mut stdout = tokio::spawn(drain(
        child.stdout.take().context("command has no stdout")?,
        maximum,
    ));
    let mut stderr = tokio::spawn(drain(
        child.stderr.take().context("command has no stderr")?,
        maximum,
    ));
    let status = timeout(Duration::from_secs(u64::from(seconds)), child.wait()).await;
    let timed_out = status.is_err();
    let success = matches!(&status, Ok(Ok(status)) if status.success());
    // The same guard also cleans up when this future is cancelled during Agent shutdown.
    drop(cleanup);
    if timed_out {
        let _ = child.kill().await;
    }
    let collected = timeout(Duration::from_secs(5), async { (&mut stdout).await? }).await;
    if collected.is_err() {
        stdout.abort();
        stderr.abort();
    }
    let (out, cut_out) = collected.context("command stdout did not close")??;
    let collected = timeout(Duration::from_secs(5), &mut stderr).await;
    if collected.is_err() {
        stderr.abort();
    }
    let (err, cut_err) = collected.context("command stderr did not close")???;
    if let Ok(status) = status {
        status?;
    }
    Ok(Execution {
        output: CommandOutput {
            success,
            stdout: out,
            stderr: err,
        },
        timed_out,
        truncated: cut_out || cut_err,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn command_output_normalizes_nul_before_enforcing_utf8_byte_limit() -> Result<()> {
        let (text, truncated) = drain(&b"a\0b\0c"[..], 8).await?;
        assert_eq!(text, "a\u{fffd}b\u{fffd}");
        assert_eq!(text.len(), 8);
        assert!(truncated);
        assert!(!text.contains('\0'));
        Ok(())
    }
    #[tokio::test]
    async fn managed_process_preserves_terminal_retirement_exit_code() -> Result<()> {
        let mut child = spawn(Path::new("sh"), &["-c".into(), "exit 78".into()])?;
        assert_eq!(child.exit_code(), None);
        let success = timeout(Duration::from_secs(5), async {
            loop {
                if let Some(success) = child.try_wait()? {
                    return Ok::<_, anyhow::Error>(success);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        assert!(!success);
        assert_eq!(child.exit_code(), Some(78));
        // Tokio returns the same terminal status on subsequent observations.
        assert_eq!(child.try_wait()?, Some(false));
        assert_eq!(child.exit_code(), Some(78));
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_a_command_terminates_its_descendants() -> Result<()> {
        let root = std::env::temp_dir().join(format!("sn-command-{}", Uuid::new_v4()));
        std::fs::create_dir(&root)?;
        let ready = root.join("ready");
        let escaped = root.join("escaped");
        let script = format!(
            "touch '{}'; sleep 2; touch '{}'",
            ready.display(),
            escaped.display()
        );
        let task = tokio::spawn(async move {
            execute(Path::new("sh"), &["-c".into(), script], 10, 1024).await
        });
        timeout(Duration::from_secs(5), async {
            while !ready.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        task.abort();
        let _ = task.await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        ensure!(
            !escaped.exists(),
            "command descendant survived cancellation"
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[tokio::test]
    async fn command_timeout_kills_descendants_and_output_is_bounded() -> Result<()> {
        let timed = execute(
            Path::new("sh"),
            &["-c".into(), "sleep 30 & wait".into()],
            1,
            1024,
        )
        .await?;
        assert!(timed.timed_out && !timed.output.success);
        let output = execute(
            Path::new("sh"),
            &["-c".into(), "yes abc | head -c 100000".into()],
            5,
            64,
        )
        .await?;
        assert!(output.output.success && output.truncated);
        assert_eq!(output.output.stdout.len(), 64);
        Ok(())
    }
}

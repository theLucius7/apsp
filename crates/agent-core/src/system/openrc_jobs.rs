use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Started {
    pid: u32,
}

impl SystemServiceManager {
    pub(super) async fn openrc_running_units(&self) -> Result<Vec<String>> {
        let entries = match fs::read_dir(&self.job_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut units = Vec::new();
        for (count, entry) in entries.enumerate() {
            ensure!(count < 1024, "诊断历史过多，请先归档旧任务后重试");
            let entry = entry?;
            let unit = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("invalid diagnostic service name"))?;
            if !unit.starts_with("apsp-diagnostic-") {
                continue;
            }
            ensure!(
                super::jobs::valid_job_unit(&unit) && entry.file_type()?.is_dir(),
                "invalid diagnostic job directory"
            );
            if self.openrc_job_status(&unit).await? == JobStatus::Running {
                units.push(unit);
            }
        }
        Ok(units)
    }
    pub(super) async fn openrc_job_status(&self, unit: &str) -> Result<JobStatus> {
        let directory = self.job_root.join(unit);
        if !directory.join("job.json").try_exists()? {
            return Ok(JobStatus::Missing);
        }
        if directory.join("result.json").try_exists()? {
            return Ok(serde_json::from_slice(&read_small(
                &directory.join("result.json"),
            )?)?);
        }
        if directory.join("started.json").try_exists()? {
            let started: Started =
                serde_json::from_slice(&read_small(&directory.join("started.json"))?)?;
            if let Ok(command) = fs::read(format!("/proc/{}/cmdline", started.pid))
                && command
                    .split(|c| *c == 0)
                    .any(|part| part == directory.join("job.json").as_os_str().as_encoded_bytes())
            {
                return Ok(JobStatus::Running);
            }
            return Ok(JobStatus::Failed {
                error: "diagnostic runner stopped without a result".into(),
            });
        }
        if fs::metadata(directory.join("job.json"))?
            .modified()?
            .elapsed()
            .unwrap_or_default()
            < Duration::from_secs(10)
        {
            return Ok(JobStatus::Running);
        }
        Ok(JobStatus::Failed {
            error: "diagnostic runner did not start".into(),
        })
    }
}

fn read_small(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 64 * 1024,
        "invalid diagnostic state file"
    );
    Ok(fs::read(path)?)
}

pub async fn run_job(spec: &Path) -> Result<()> {
    let ops = SystemOps;
    let directory = spec.parent().context("job has no parent")?;
    let job: ServiceJob = serde_json::from_slice(&read_small(spec)?)?;
    ensure!(
        job.program.is_absolute()
            && job.working_directory.is_absolute()
            && (1..=3600).contains(&job.timeout_secs),
        "invalid diagnostic job"
    );
    super::jobs::prepare_diagnostic_lock(&ops).await?;
    // The exclusive journal entry makes repeated manual invocation fail closed.
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut started = options.open(directory.join("started.json"))?;
    started.write_all(&serde_json::to_vec(&Started {
        pid: std::process::id(),
    })?)?;
    started.sync_all()?;
    std::env::set_current_dir(&job.working_directory)?;
    let mut args = vec![
        "--exclusive".into(),
        "--nonblock".into(),
        "--conflict-exit-code=75".into(),
        super::jobs::DIAGNOSTIC_LOCK_PATH.into(),
        "unshare".into(),
        "--mount".into(),
        "--propagation".into(),
        "private".into(),
        "--".into(),
        job.program.to_string_lossy().into_owned(),
    ];
    args.extend(job.args);
    let output = ops
        .execute_bounded(Path::new("flock"), &args, job.timeout_secs, 64 * 1024)
        .await;
    let status = match output {
        Ok(output) if output.output.success && !output.timed_out => JobStatus::Succeeded,
        Ok(output) if output.timed_out => JobStatus::Failed {
            error: "diagnostic service failed: timeout".into(),
        },
        Ok(output) => JobStatus::Failed {
            error: format!(
                "diagnostic command failed: {}",
                output.output.stderr.chars().take(1024).collect::<String>()
            ),
        },
        Err(error) => JobStatus::Failed {
            error: error.to_string(),
        },
    };
    ops.write_file(
        &directory.join("result.json"),
        &serde_json::to_vec(&status)?,
        0o600,
        None,
    )
    .await?;
    ensure!(status == JobStatus::Succeeded, "diagnostic job failed");
    Ok(())
}

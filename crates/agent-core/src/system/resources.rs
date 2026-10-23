use super::*;
use apsp_adapter_sdk::{DiagnosticMemory, DiagnosticResources};
use std::path::{Component, PathBuf};
use tokio::io::AsyncReadExt;

const MAX_RESOURCE_BYTES: u64 = 64 * 1024;

async fn bounded_file(path: &Path) -> Result<String> {
    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_RESOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .await?;
    ensure!(
        bytes.len() as u64 <= MAX_RESOURCE_BYTES,
        "resource file exceeds its size limit"
    );
    Ok(String::from_utf8(bytes)?)
}

fn host_memory(text: &str) -> Result<u64> {
    let fields: Vec<_> = text
        .lines()
        .find(|line| line.starts_with("MemAvailable:"))
        .context("MemAvailable is missing")?
        .split_whitespace()
        .collect();
    ensure!(
        fields.len() == 3 && fields[2] == "kB",
        "MemAvailable has unexpected units"
    );
    fields[1]
        .parse::<u64>()?
        .checked_mul(1024)
        .context("MemAvailable overflow")
}

fn cgroup_path(text: &str) -> Result<PathBuf> {
    let path = text
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .context("diagnostic safety requires cgroup v2")?;
    let path = Path::new(path);
    ensure!(
        path.is_absolute()
            && path
                .components()
                .all(|part| matches!(part, Component::RootDir | Component::Normal(_))),
        "cgroup path contains traversal"
    );
    Ok(Path::new("/sys/fs/cgroup").join(path.strip_prefix("/")?))
}

fn effective_cgroup_headroom(values: &[(String, String)]) -> Result<Option<u64>> {
    let mut available = None;
    for (maximum, current) in values {
        let current: u64 = current
            .trim()
            .parse()
            .context("invalid cgroup memory.current")?;
        if maximum.trim() == "max" {
            continue;
        }
        let maximum: u64 = maximum
            .trim()
            .parse()
            .context("invalid cgroup memory.max")?;
        let headroom = maximum.saturating_sub(current);
        available = Some(available.map_or(headroom, |previous: u64| previous.min(headroom)));
    }
    Ok(available)
}

pub(super) async fn memory() -> Result<DiagnosticMemory> {
    ensure!(
        cfg!(target_os = "linux"),
        "diagnostic safety requires Linux"
    );
    let host_available_bytes = host_memory(&bounded_file(Path::new("/proc/meminfo")).await?)?;
    let directory = cgroup_path(&bounded_file(Path::new("/proc/self/cgroup")).await?)?;
    let root = Path::new("/sys/fs/cgroup");
    let mut values = Vec::new();
    for ancestor in directory
        .ancestors()
        .take_while(|ancestor| ancestor.starts_with(root))
    {
        let maximum = ancestor.join("memory.max");
        if ancestor == root && !tokio::fs::try_exists(&maximum).await? {
            // The host root is unlimited and does not expose memory.max.
            continue;
        }
        values.push((
            bounded_file(&maximum).await?,
            bounded_file(&ancestor.join("memory.current")).await?,
        ));
    }
    Ok(DiagnosticMemory {
        host_available_bytes,
        cgroup_available_bytes: effective_cgroup_headroom(&values)?,
    })
}

fn validate_directory(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && path
                .components()
                .all(|part| matches!(part, Component::RootDir | Component::Normal(_))),
        "diagnostic directory contains traversal"
    );
    for ancestor in path.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "diagnostic directory has a symbolic link ancestor"
        );
    }
    Ok(())
}

fn disk_available(output: &CommandOutput) -> Result<u64> {
    ensure!(
        output.success && output.stdout.len() <= 4096,
        "cannot inspect available diagnostic disk space"
    );
    let fields: Vec<_> = output.stdout.split_whitespace().collect();
    ensure!(fields.len() == 2, "unexpected available disk response");
    let blocks: u64 = fields[0].parse()?;
    let block_size: u64 = fields[1].parse()?;
    ensure!(block_size > 0, "invalid filesystem block size");
    blocks
        .checked_mul(block_size)
        .context("available disk size overflow")
}

pub(super) async fn snapshot(ops: &SystemOps, directory: &Path) -> Result<DiagnosticResources> {
    validate_directory(directory)?;
    let memory = memory().await?;
    let output = ops
        .execute_bounded(
            Path::new("stat"),
            &[
                "-f".into(),
                "-c".into(),
                "%a %S".into(),
                "--".into(),
                directory
                    .to_str()
                    .context("diagnostic directory is not UTF-8")?
                    .into(),
            ],
            5,
            4096,
        )
        .await?;
    ensure!(
        !output.timed_out && !output.truncated,
        "available disk inspection timed out or exceeded its response limit"
    );
    let load_one = bounded_file(Path::new("/proc/loadavg"))
        .await?
        .split_whitespace()
        .next()
        .context("load average is missing")?
        .parse()?;
    let cpu_count = u32::try_from(std::thread::available_parallelism()?.get())?;
    Ok(DiagnosticResources {
        memory,
        disk_available_bytes: disk_available(&output.output)?,
        load_one,
        cpu_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_memory_respects_every_ancestor_and_host_availability() -> Result<()> {
        assert_eq!(host_memory("MemAvailable: 1024 kB\n")?, 1024 * 1024);
        assert!(host_memory("MemAvailable: 1024 MB\n").is_err());
        assert!(host_memory("MemFree: 1024 kB\n").is_err());
        let values = [
            ("1000".into(), "200".into()),
            ("600".into(), "300".into()),
            ("max".into(), "400".into()),
        ];
        let headroom = effective_cgroup_headroom(&values)?;
        assert_eq!(headroom, Some(300));
        assert_eq!(
            DiagnosticMemory {
                host_available_bytes: 200,
                cgroup_available_bytes: headroom
            }
            .available_bytes(),
            200
        );
        assert_eq!(
            effective_cgroup_headroom(&[("10".into(), "20".into())])?,
            Some(0)
        );
        assert!(effective_cgroup_headroom(&[("bad".into(), "1".into())]).is_err());
        assert!(effective_cgroup_headroom(&[("max".into(), "bad".into())]).is_err());
        Ok(())
    }

    #[test]
    fn resource_paths_and_disk_responses_fail_closed() -> Result<()> {
        assert_eq!(
            cgroup_path("0::/system.slice/example.service\n")?,
            Path::new("/sys/fs/cgroup/system.slice/example.service")
        );
        for path in ["0::/../outside", "0::relative", "1:memory:/system.slice"] {
            assert!(cgroup_path(path).is_err());
        }
        let response = |success, stdout: &str| CommandOutput {
            success,
            stdout: stdout.into(),
            stderr: String::new(),
        };
        assert_eq!(disk_available(&response(true, "1234 4096\n"))?, 1234 * 4096);
        assert_eq!(disk_available(&response(true, "0 4096\n"))?, 0);
        for output in [
            response(false, "1234 4096\n"),
            response(true, "bad 4096\n"),
            response(true, "1234 4096 extra\n"),
            response(true, "1234 0\n"),
            response(true, "18446744073709551615 4096\n"),
        ] {
            assert!(disk_available(&output).is_err());
        }
        Ok(())
    }
}

use super::*;
use std::path::{Component, PathBuf};
use tokio::io::AsyncReadExt;

pub(super) fn supported(backend: ServiceBackend) -> bool {
    supported_at(backend, Path::new("/sys/fs/cgroup/cgroup.controllers"))
}

fn supported_at(backend: ServiceBackend, controllers: &Path) -> bool {
    cfg!(target_os = "linux") && backend == ServiceBackend::Systemd && controllers.is_file()
}

async fn read_bounded(path: &Path) -> Result<String> {
    let file = tokio::fs::File::open(path).await?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes).await?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "cleanup evidence exceeds its size limit"
    );
    Ok(String::from_utf8(bytes)?)
}

fn safe_directory(directory: &Path) -> Result<()> {
    ensure!(
        directory.is_absolute()
            && directory
                .components()
                .all(|part| matches!(part, Component::RootDir | Component::Normal(_))),
        "unsafe diagnostic cleanup directory"
    );
    for ancestor in directory.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "diagnostic cleanup path has a symbolic link"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn mount_path(value: &str) -> Result<PathBuf> {
    let mut decoded = Vec::new();
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'\\' {
            let mut octal = 0_u16;
            for _ in 0..3 {
                let digit = bytes.next().context("incomplete mount path escape")?;
                ensure!((b'0'..=b'7').contains(&digit), "invalid mount path escape");
                octal = octal * 8 + u16::from(digit - b'0');
            }
            decoded.push(u8::try_from(octal)?);
        } else {
            decoded.push(byte);
        }
    }
    Ok(PathBuf::from(String::from_utf8(decoded)?))
}

fn workspace_has_mounts(text: &str, directory: &Path) -> Result<bool> {
    for line in text.lines() {
        let target = line
            .split_whitespace()
            .nth(4)
            .context("invalid mountinfo row")?;
        if mount_path(target)?.starts_with(directory) {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn cgroup_empty(group: &str, unit: &str) -> Result<bool> {
    if group.is_empty() {
        return Ok(true);
    }
    let group = Path::new(group);
    ensure!(
        group.is_absolute()
            && group.file_name() == Some(std::ffi::OsStr::new(unit))
            && group
                .components()
                .all(|part| matches!(part, Component::RootDir | Component::Normal(_))),
        "unknown diagnostic cgroup identity"
    );
    let mut directories = vec![Path::new("/sys/fs/cgroup").join(group.strip_prefix("/")?)];
    let mut visited = 0;
    while let Some(directory) = directories.pop() {
        visited += 1;
        ensure!(visited <= 4096, "diagnostic cgroup subtree is too large");
        let processes = match read_bounded(&directory.join("cgroup.procs")).await {
            Ok(value) => value,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        if !processes.trim().is_empty() {
            return Ok(false);
        }
        let mut entries = tokio::fs::read_dir(directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            let kind = entry.file_type().await?;
            ensure!(
                !kind.is_symlink(),
                "diagnostic cgroup contains a symbolic link"
            );
            if kind.is_dir() {
                directories.push(entry.path());
            }
            ensure!(
                directories.len() + visited <= 4096,
                "diagnostic cgroup subtree is too large"
            );
        }
    }
    Ok(true)
}

impl SystemServiceManager {
    pub(super) async fn confirm_diagnostic_cleanup(
        &self,
        unit: &str,
        directory: &Path,
    ) -> Result<bool> {
        ensure!(supported(self.backend), "当前服务后端不支持诊断清理确认");
        ensure!(
            super::jobs::valid_job_unit(unit),
            "invalid diagnostic cleanup unit"
        );
        safe_directory(directory)?;
        let output = self.privileged.execute_bounded(Path::new("systemctl"), &[
            "show".into(), "--property=LoadState,ActiveState,MainPID,ControlPID,ControlGroup,PrivateMounts,KillMode".into(), "--".into(), unit.into(),
        ], 5, 16 * 1024).await?;
        ensure!(
            !output.timed_out && !output.truncated,
            "诊断清理状态查询超时或超限"
        );
        if super::services::parse_runtime_active(&output.output)? {
            return Ok(false);
        }
        let properties: std::collections::BTreeMap<_, _> = output
            .output
            .stdout
            .lines()
            .filter_map(|line| line.split_once('='))
            .collect();
        if matches!(properties.get("LoadState"), Some(&"loaded" | &"masked")) {
            ensure!(
                properties.get("PrivateMounts") == Some(&"yes")
                    && properties.get("KillMode") == Some(&"control-group"),
                "无法证明此旧诊断单元的进程与私有挂载已清理"
            );
        }
        let group = properties
            .get("ControlGroup")
            .context("diagnostic cgroup evidence is missing")?;
        if !cgroup_empty(group, unit).await? {
            return Ok(false);
        }
        Ok(!workspace_has_mounts(
            &read_bounded(Path::new("/proc/self/mountinfo")).await?,
            directory,
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmation_capability_rejects_missing_v2_evidence_and_other_backends() -> Result<()> {
        let directory =
            std::env::temp_dir().join(format!("apsp-cleanup-capability-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory)?;
        let controllers = directory.join("cgroup.controllers");
        assert!(!supported_at(ServiceBackend::Systemd, &controllers));
        std::fs::write(&controllers, "memory pids\n")?;
        assert_eq!(
            supported_at(ServiceBackend::Systemd, &controllers),
            cfg!(target_os = "linux")
        );
        for backend in [
            ServiceBackend::OpenRc,
            ServiceBackend::Launchd,
            ServiceBackend::FreeBsd,
            ServiceBackend::WindowsTask,
            ServiceBackend::Unmanaged,
        ] {
            assert!(!supported_at(backend, &controllers));
        }
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn mount_evidence_decodes_paths_and_keeps_directory_boundaries() -> Result<()> {
        let directory = Path::new("/srv/diagnostic job");
        assert!(workspace_has_mounts(
            "1 0 0:1 / /srv/diagnostic\\040job/rootfs rw - tmpfs tmpfs rw",
            directory
        )?);
        assert!(!workspace_has_mounts(
            "1 0 0:1 / /srv/diagnostic\\040job-other rw - tmpfs tmpfs rw",
            directory
        )?);
        assert!(workspace_has_mounts("invalid", directory).is_err());
        assert!(mount_path("/bad\\xyz").is_err());
        assert!(safe_directory(Path::new("/srv/../outside")).is_err());
        Ok(())
    }
}

#[cfg(test)]
mod acceptance;

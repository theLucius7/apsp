use anyhow::{Context, Result, ensure};
use std::{
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
};

pub(super) const DIRECTORY: &str = "/run/apsp/runtime-logs";
const MARKER: &str = "# APSP_RUNTIME_LOG_V1=memory-socket";

fn service_name(unit: &str) -> Result<&str> {
    let service = unit
        .strip_suffix(".service")
        .context("invalid runtime log unit")?;
    ensure!(
        !service.is_empty()
            && !matches!(service, "." | "..")
            && !service.starts_with('-')
            && unit.len() <= 80
            && service
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'.' | b'_' | b'-' | b'@')),
        "invalid runtime log unit"
    );
    Ok(service)
}

pub(super) fn socket_path(unit: &str, stream: &str) -> Result<PathBuf> {
    service_name(unit)?;
    ensure!(
        matches!(stream, "stdout" | "stderr"),
        "invalid runtime log stream"
    );
    let path = Path::new(DIRECTORY)
        .join(unit)
        .join(format!("{stream}.sock"));
    ensure!(
        path.as_os_str().len() < 108,
        "runtime log socket path is too long"
    );
    Ok(path)
}

/// Parents are root-owned; only the final private directory belongs to the
/// runtime account. Log readers also verify the connected process's UID.
pub(super) fn directory_owner(socket: &Path) -> Result<u32> {
    let directory = socket
        .parent()
        .context("runtime log socket has no parent")?;
    let metadata = fs::symlink_metadata(directory)?;
    ensure!(
        metadata.is_dir() && metadata.uid() != 0 && metadata.mode() & 0o077 == 0,
        "runtime log directory must be private to the runtime account"
    );
    for parent in directory.ancestors().skip(1) {
        let metadata = fs::symlink_metadata(parent)?;
        ensure!(
            metadata.is_dir() && metadata.uid() == 0 && metadata.mode() & 0o022 == 0,
            "runtime log parent must be protected by root"
        );
    }
    Ok(metadata.uid())
}

pub(super) fn socket_owner(path: &Path) -> Result<u32> {
    let owner = directory_owner(path)?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.file_type().is_socket() && metadata.uid() == owner && metadata.mode() & 0o077 == 0,
        "runtime log socket has an unexpected owner or permissions"
    );
    Ok(owner)
}

pub(crate) fn available(unit: &str) -> bool {
    let Ok(service) = service_name(unit) else {
        return false;
    };
    if socket_path(unit, "stdout").is_err() || !Path::new("/run/openrc/softlevel").is_file() {
        return false;
    }
    let path = Path::new("/etc/init.d").join(service);
    protected_contract(&path).unwrap_or(false)
}

fn protected_contract(path: &Path) -> Result<bool> {
    for parent in path.ancestors().skip(1) {
        let metadata = fs::symlink_metadata(parent)?;
        ensure!(
            metadata.is_dir() && metadata.uid() == 0 && metadata.mode() & 0o022 == 0,
            "runtime log service parent is not protected"
        );
    }
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file()
            && metadata.uid() == 0
            && metadata.mode() & 0o022 == 0
            && metadata.len() <= 16 * 1024,
        "runtime log service is not protected"
    );
    let text = fs::read_to_string(path)?;
    Ok(has_contract(&text))
}

pub(super) fn has_contract(text: &str) -> bool {
    text.lines().any(|line| line == MARKER)
        && ["stdout", "stderr"].iter().all(|stream| {
            let variable = if *stream == "stdout" { "output_logger" } else { "error_logger" };
            let expected = format!("{variable}=\"/opt/apsp/core/current/apsp-agent runtime-log --unit ${{RC_SVCNAME}}.service --stream {stream}\"");
            text.lines().any(|line| line == expected)
        })
        && !text.lines().any(|line| line.starts_with("output_log=") || line.starts_with("error_log="))
}

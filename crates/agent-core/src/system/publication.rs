use super::{parent_directory, sync_directory};
use crate::artifacts::safe_component;
use anyhow::{Result, ensure};
#[cfg(not(target_os = "linux"))]
use apsp_adapter_sdk::CommandOutput;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) fn is_request(program: &Path, args: &[String]) -> bool {
    program == Path::new("/bin/mv")
        && args.len() == 5
        && args[..3] == ["--no-clobber", "--no-target-directory", "--"]
}

pub(super) fn validate(args: &[String]) -> Result<()> {
    let source = Path::new(&args[3]);
    let destination = Path::new(&args[4]);
    ensure!(
        source.is_absolute()
            && source.parent() == destination.parent()
            && source
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(".verified-"))
            && destination
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(safe_component),
        "invalid artifact publication paths"
    );
    ensure!(
        fs::symlink_metadata(source)?.is_dir()
            && fs::symlink_metadata(parent_directory(source)?)?.is_dir(),
        "artifact publication requires ordinary directories"
    );
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(super) async fn simulate(args: &[String]) -> Result<CommandOutput> {
    // All native publications cooperate through an exclusive sibling lock.
    // This also prevents Unix rename from replacing a concurrently created
    // empty version directory in another Agent installation.
    let source = PathBuf::from(&args[3]);
    let destination = PathBuf::from(&args[4]);
    tokio::task::spawn_blocking(move || -> Result<CommandOutput> {
        let parent = parent_directory(&destination)?;
        let lock = parent.join(format!(
            ".publish-{}.lock",
            destination
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| anyhow::anyhow!("invalid artifact destination"))?
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let guard = options
            .open(&lock)
            .map_err(|error| anyhow::anyhow!("cannot lock artifact publication: {error}"))?;
        let result = (|| -> Result<CommandOutput> {
            ensure!(
                fs::symlink_metadata(&destination)
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
                "artifact destination already exists"
            );
            fs::rename(&source, &destination)?;
            sync_directory(&destination)?;
            sync_directory(parent_directory(&destination)?)?;
            Ok(CommandOutput {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            })
        })();
        drop(guard);
        fs::remove_file(&lock)?;
        result
    })
    .await?
}

pub(super) async fn sync(args: &[String]) -> Result<()> {
    let destination = PathBuf::from(&args[4]);
    tokio::task::spawn_blocking(move || -> Result<()> {
        ensure!(
            fs::symlink_metadata(&destination)?.is_dir(),
            "published artifact is not an ordinary directory"
        );
        sync_directory(&destination)?;
        sync_directory(parent_directory(&destination)?)
    })
    .await?
}

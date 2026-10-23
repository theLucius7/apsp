use anyhow::{Context, Result, ensure};
use std::{
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    fs,
    io::AsyncReadExt,
    time::{Instant, timeout_at},
};

pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn now_millis() -> Result<u64> {
    Ok(u64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
    )?)
}

fn owner(value: &std::fs::Metadata) -> Result<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(value.uid())
    }
    #[cfg(not(unix))]
    {
        let _ = value;
        anyhow::bail!("private workspace verification requires Unix")
    }
}

async fn metadata(
    path: &Path,
    directory: bool,
    private: bool,
    expected_owner: Option<u32>,
) -> Result<std::fs::Metadata> {
    let value = fs::symlink_metadata(path).await?;
    ensure!(
        !value.file_type().is_symlink()
            && if directory {
                value.is_dir()
            } else {
                value.is_file()
            },
        "diagnostic path must be an ordinary directory/file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            !private || value.permissions().mode() & 0o077 == 0,
            "diagnostic data must be private"
        );
        ensure!(
            directory || value.nlink() == 1,
            "diagnostic hardlinks are not allowed"
        );
    }
    if let Some(expected) = expected_owner {
        ensure!(
            owner(&value)? == expected,
            "diagnostic owner differs from its trusted workspace"
        );
    }
    #[cfg(not(unix))]
    {
        let _ = private;
        anyhow::bail!("private workspace verification requires Unix");
    }
    #[cfg(unix)]
    Ok(value)
}
pub(crate) async fn ancestors(path: &Path, deadline: Instant) -> Result<()> {
    timeout_at(deadline.min(Instant::now() + IO_TIMEOUT), async {
        for parent in path.ancestors() {
            match fs::symlink_metadata(parent).await {
                Ok(value) => ensure!(
                    value.is_dir() && !value.file_type().is_symlink(),
                    "workspace ancestor is not an ordinary directory"
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    })
    .await
    .context("workspace ancestor inspection timed out")?
}
pub(crate) async fn workspace(
    path: &Path,
    expected_owner: Option<u32>,
    deadline: Instant,
) -> Result<Option<u32>> {
    timeout_at(deadline.min(Instant::now() + IO_TIMEOUT), async {
        match fs::symlink_metadata(path).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
            Ok(_) => (),
        }
        let metadata = metadata(path, true, true, expected_owner).await?;
        ensure!(
            fs::canonicalize(path).await? == path,
            "workspace ancestors must not be symlinks"
        );
        Ok(Some(owner(&metadata)?))
    })
    .await
    .context("workspace inspection timed out")?
}
pub(crate) async fn binary(path: &Path, deadline: Instant) -> Result<u32> {
    timeout_at(deadline.min(Instant::now() + IO_TIMEOUT), async {
        let metadata = metadata(path, false, false, None).await?;
        ensure!(
            fs::canonicalize(path).await? == path,
            "binary ancestors must not be symlinks"
        );
        owner(&metadata)
    })
    .await
    .context("binary inspection timed out")?
}
pub(crate) async fn directory(path: &Path, expected_owner: u32, deadline: Instant) -> Result<bool> {
    timeout_at(deadline.min(Instant::now() + IO_TIMEOUT), async {
        match fs::symlink_metadata(path).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
            Ok(_) => (),
        }
        metadata(path, true, true, Some(expected_owner)).await?;
        Ok(true)
    })
    .await
    .context("section directory inspection timed out")?
}
pub(crate) async fn read(
    path: &Path,
    limit: usize,
    expected_owner: u32,
    deadline: Instant,
) -> Result<Option<String>> {
    timeout_at(deadline.min(Instant::now() + IO_TIMEOUT), async {
        let before = match metadata(path, false, true, Some(expected_owner)).await {
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
            Ok(value) => value,
        };
        ensure!(
            before.len() <= limit as u64,
            "diagnostic file exceeds its limit"
        );
        let file = fs::File::open(path).await?;
        let opened = file.metadata().await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            ensure!(
                before.ino() == opened.ino()
                    && before.dev() == opened.dev()
                    && opened.nlink() == 1
                    && opened.uid() == expected_owner
                    && opened.permissions().mode() & 0o077 == 0,
                "diagnostic file identity changed"
            );
        }
        ensure!(
            opened.is_file() && opened.len() <= limit as u64,
            "diagnostic file must be bounded and ordinary"
        );
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1).read_to_end(&mut bytes).await?;
        ensure!(bytes.len() <= limit, "diagnostic file exceeds its limit");
        Ok(Some(String::from_utf8(bytes)?))
    })
    .await
    .context("diagnostic file read timed out")?
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        io::Write,
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn private_files_and_directories_reject_a_different_trusted_owner() {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "apsp-tcp-owner-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let fixture = Fixture(root);
        let path = fixture.0.join("targets.json");
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        output.write_all(b"saved private fixture").unwrap();
        let actual = output.metadata().unwrap().uid();
        let other = actual.wrapping_add(1);
        let deadline = Instant::now() + Duration::from_secs(5);
        assert_eq!(binary(&path, deadline).await.unwrap(), actual);
        assert_eq!(
            workspace(&fixture.0, Some(actual), deadline).await.unwrap(),
            Some(actual)
        );
        assert!(directory(&fixture.0, actual, deadline).await.unwrap());
        assert_eq!(
            read(&path, 128, actual, deadline).await.unwrap().as_deref(),
            Some("saved private fixture")
        );
        for error in [
            workspace(&fixture.0, Some(other), deadline)
                .await
                .unwrap_err(),
            directory(&fixture.0, other, deadline).await.unwrap_err(),
            read(&path, 128, other, deadline).await.unwrap_err(),
        ] {
            assert!(format!("{error:#}").contains("owner"));
        }
    }
}

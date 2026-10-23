use super::*;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Debug)]
enum Call {
    Directory(PathBuf, u32, Option<String>),
    Execute(PathBuf, Vec<String>),
    Write(PathBuf),
}

struct RecordingOps {
    metadata: &'static str,
    calls: Mutex<Vec<Call>>,
}

impl Privileged for RecordingOps {
    fn execute<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
    ) -> BoxFuture<'a, CommandOutput> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Execute(program.into(), args.to_vec()));
            Ok(CommandOutput {
                success: true,
                stdout: if program == Path::new("stat") {
                    self.metadata.into()
                } else if let Some(output) =
                    crate::system::syscall_protection::fixture_output(program, args)
                {
                    output.into()
                } else {
                    String::new()
                },
                ..Default::default()
            })
        })
    }
    fn create_dir<'a>(
        &'a self,
        path: &'a Path,
        mode: u32,
        group: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(Call::Directory(
                path.into(),
                mode,
                group.map(str::to_owned),
            ));
            Ok(())
        })
    }
    fn write_file<'a>(
        &'a self,
        path: &'a Path,
        _bytes: &'a [u8],
        _: u32,
        _: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(Call::Write(path.into()));
            Ok(())
        })
    }
    fn atomic_symlink<'a>(&'a self, _: &'a Path, _: &'a Path) -> BoxFuture<'a, ()> {
        panic!("unexpected symlink creation")
    }
    fn remove_symlink<'a>(&'a self, _: &'a Path) -> BoxFuture<'a, ()> {
        panic!("unexpected symlink removal")
    }
    fn install_archive<'a>(&'a self, _: &'a Path, _: &'a Path, _: &'a str) -> BoxFuture<'a, ()> {
        panic!("unexpected archive installation")
    }
}

fn job() -> Result<ServiceJob> {
    Ok(serde_json::from_value(serde_json::json!({
        "unit": format!("apsp-diagnostic-{}.service", Uuid::new_v4()),
        "program": "/usr/bin/true",
        "args": [],
        "working_directory": "/tmp",
        "timeout_secs": 30,
    }))?)
}

#[tokio::test]
async fn unsafe_lock_directory_metadata_prevents_service_start() -> Result<()> {
    for metadata in [
        "41c0 1000",
        "41ed 0",
        "a1c0 0",
        "81c0 0",
        "41c0 0 extra",
        "garbage",
    ] {
        let backend = ServiceBackend::Systemd;
        let ops = Arc::new(RecordingOps {
            metadata,
            calls: Mutex::new(Vec::new()),
        });
        let services = SystemServiceManager::new(ops.clone(), backend)
            .with_job_root(std::env::temp_dir().join(format!("apsp-lock-{}", Uuid::new_v4())));
        assert!(
            services.start_job(&job()?).await.is_err(),
            "{backend:?}: {metadata}"
        );
        let calls = ops.calls.lock().unwrap();
        assert!(
            matches!(&calls[0], Call::Directory(path, 0o700, Some(group))
            if path == Path::new(DIAGNOSTIC_LOCK_DIRECTORY) && group == "root")
        );
        assert!(calls.iter().all(|call| match call {
            Call::Directory(..) => true,
            Call::Execute(program, _) => program == Path::new("stat"),
            Call::Write(..) => false,
        }));
    }
    Ok(())
}

#[tokio::test]
async fn service_starts_prepare_private_lock_and_private_umask() -> Result<()> {
    let backend = ServiceBackend::Systemd;
    let ops = Arc::new(RecordingOps {
        metadata: "41c0 0",
        calls: Mutex::new(Vec::new()),
    });
    let services = SystemServiceManager::new(ops.clone(), backend)
        .with_job_root(std::env::temp_dir().join(format!("apsp-lock-{}", Uuid::new_v4())));
    services.start_job(&job()?).await?;
    let calls = ops.calls.lock().unwrap();
    assert!(
        matches!(&calls[0], Call::Directory(path, 0o700, Some(group))
        if path == Path::new(DIAGNOSTIC_LOCK_DIRECTORY) && group == "root")
    );
    assert!(matches!(&calls[1], Call::Execute(program, args)
        if program == Path::new("stat") && args == &["-c", "%f %u", "--", DIAGNOSTIC_LOCK_DIRECTORY]));
    assert!(
        calls.iter().all(
            |call| !matches!(call, Call::Write(path) if path == Path::new(DIAGNOSTIC_LOCK_PATH))
        )
    );
    let Call::Execute(program, args) = calls.last().unwrap() else {
        panic!("missing launch")
    };
    assert_eq!(program, Path::new("systemd-run"));
    assert!(args.iter().any(|arg| arg == DIAGNOSTIC_LOCK_PATH));
    assert!(args.iter().any(|arg| arg == "--property=UMask=0077"));
    Ok(())
}

#[tokio::test]
#[ignore = "requires Linux, Python 3, flock, and root"]
async fn real_systemd_diagnostic_lock_blocks_unprivileged_open_and_preserves_held_inode()
-> Result<()> {
    use std::os::unix::fs::MetadataExt;
    ensure!(cfg!(target_os = "linux"), "requires Linux");
    let ops = SystemOps;
    prepare_diagnostic_lock(&ops).await?;
    let directory = std::env::temp_dir().join(format!("apsp-lock-test-{}", Uuid::new_v4()));
    fs::create_dir(&directory)?;
    let ready = directory.join("ready");
    let mut holder = ops.spawn_managed(Path::new("python3"), &[
        "-c".into(),
        "import fcntl, os, sys, time\nfd = os.open(sys.argv[1], os.O_CREAT | os.O_RDWR, 0o644)\nos.fchmod(fd, 0o644)\nfcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)\nopen(sys.argv[2], 'w').close()\ntime.sleep(30)".into(),
        DIAGNOSTIC_LOCK_PATH.into(), ready.to_string_lossy().into_owned(),
    ]).await?;
    let result = async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !ready.try_exists()? {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok::<_, anyhow::Error>(())
        }).await??;
        let inode = fs::metadata(DIAGNOSTIC_LOCK_PATH)?.ino();
        prepare_diagnostic_lock(&ops).await?;
        assert_eq!(fs::metadata(DIAGNOSTIC_LOCK_PATH)?.ino(), inode);
        let denied = ops.execute_bounded(Path::new("python3"), &[
            "-c".into(),
            "import os, sys\nos.setgroups([])\nos.setgid(65534)\nos.setuid(65534)\ntry:\n    os.open(sys.argv[1], os.O_RDONLY)\nexcept PermissionError:\n    sys.exit(0)\nsys.exit(1)".into(),
            DIAGNOSTIC_LOCK_PATH.into(),
        ], 5, 1024).await?;
        assert!(denied.output.success && !denied.timed_out, "ordinary account could open diagnostic lock");
        let conflict = ops.execute_bounded(Path::new("python3"), &[
            "-c".into(),
            "import subprocess, sys\nsys.exit(0 if subprocess.run(['flock', '--exclusive', '--nonblock', '--conflict-exit-code=75', sys.argv[1], 'true']).returncode == 75 else 1)".into(),
            DIAGNOSTIC_LOCK_PATH.into(),
        ], 5, 1024).await?;
        assert!(conflict.output.success && !conflict.timed_out, "repeated preparation split the held lock");
        Ok::<_, anyhow::Error>(())
    }.await;
    holder.terminate().await?;
    fs::remove_dir_all(directory)?;
    result?;
    let released = ops
        .execute_bounded(
            Path::new("flock"),
            &[
                "--exclusive".into(),
                "--nonblock".into(),
                DIAGNOSTIC_LOCK_PATH.into(),
                "true".into(),
            ],
            5,
            1024,
        )
        .await?;
    assert!(
        released.output.success && !released.timed_out,
        "diagnostic lock did not release with the process"
    );
    Ok(())
}

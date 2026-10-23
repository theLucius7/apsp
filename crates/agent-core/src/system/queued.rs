use super::*;

async fn start_queued_fixture(
    privileged: &dyn Privileged,
    unit: &str,
    directory: &Path,
    script: &str,
    after: Option<&str>,
) -> Result<()> {
    let mut args = vec![
        format!("--unit={unit}"),
        "--no-block".into(),
        "--property=Type=oneshot".into(),
        "--property=RemainAfterExit=yes".into(),
        "--property=TimeoutStartSec=60s".into(),
        "--property=TimeoutStopSec=5s".into(),
        "--property=KillMode=control-group".into(),
        "--property=UMask=0077".into(),
        "--property=StandardOutput=null".into(),
        "--property=StandardError=journal".into(),
        format!("--property=WorkingDirectory={}", directory.display()),
    ];
    if let Some(after) = after {
        args.push(format!("--property=After={after}"));
    }
    args.extend(["--".into(), "/bin/sh".into(), script.into()]);
    let output = privileged.execute(Path::new("systemd-run"), &args).await?;
    ensure!(
        output.success,
        "cannot start queued fixture: {}",
        output.stderr
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires Linux, a running systemd system manager, and root to create isolated transient services"]
async fn real_systemd_diagnostic_queued_start_stays_running_until_executed() -> Result<()> {
    ensure!(cfg!(target_os = "linux"), "requires Linux/systemd");
    ensure!(
        Path::new("/run/systemd/system").is_dir(),
        "requires a running systemd system manager"
    );
    let privileged: Arc<dyn Privileged> = Arc::new(SystemOps);
    let identity = privileged.execute(Path::new("id"), &["-u".into()]).await?;
    ensure!(
        identity.success && identity.stdout.trim() == "0",
        "requires root for isolated transient services"
    );
    let services = SystemServiceManager::new(privileged.clone(), ServiceBackend::Systemd);
    let directory = std::env::temp_dir().join(format!("apsp-queued-test-{}", Uuid::new_v4()));
    std::fs::create_dir(&directory)?;
    let barrier = format!("apsp-diagnostic-{}.service", Uuid::new_v4());
    let target = format!("apsp-diagnostic-{}.service", Uuid::new_v4());
    let mut owned_units = Vec::new();
    let result = async {
        std::fs::write(
            directory.join("barrier.sh"),
            "set -eu\n: > barrier-started\nwhile [ ! -f release ]; do sleep 0.1; done\n",
        )?;
        std::fs::write(
            directory.join("target.sh"),
            "set -eu\nprintf 'executed\\n' > target-executed\n",
        )?;
        start_queued_fixture(
            privileged.as_ref(),
            &barrier,
            &directory,
            "barrier.sh",
            None,
        )
        .await?;
        owned_units.push(barrier.clone());
        timeout(Duration::from_secs(10), async {
            while !directory.join("barrier-started").is_file() {
                let status = services.job_status(&barrier).await?;
                ensure!(
                    status == JobStatus::Running,
                    "barrier ended early: {status:?}"
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("barrier did not begin execution")??;
        start_queued_fixture(
            privileged.as_ref(),
            &target,
            &directory,
            "target.sh",
            Some(&barrier),
        )
        .await?;
        owned_units.push(target.clone());

        // The barrier owns an unfinished start job, so After= prevents the target
        // from executing. Its inactive state must not be treated as a failure.
        let properties = privileged
            .execute(
                Path::new("systemctl"),
                &[
                    "show".into(),
                    "--property=ActiveState,ExecMainStartTimestampMonotonic".into(),
                    "--".into(),
                    target.clone(),
                ],
            )
            .await?;
        ensure!(properties.success, "cannot inspect queued fixture");
        let properties: std::collections::BTreeMap<_, _> = properties
            .stdout
            .lines()
            .filter_map(|line| line.split_once('='))
            .collect();
        ensure!(
            properties.get("ActiveState") == Some(&"inactive")
                && properties.get("ExecMainStartTimestampMonotonic") == Some(&"0"),
            "target did not remain queued before execution"
        );
        let status = services.job_status(&target).await?;
        ensure!(
            status == JobStatus::Running,
            "queued diagnostic must remain running: {status:?}"
        );
        ensure!(
            !directory.join("target-executed").exists(),
            "queued diagnostic executed before the barrier was released"
        );

        std::fs::write(directory.join("release"), [])?;
        let status = timeout(Duration::from_secs(10), async {
            loop {
                let status = services.job_status(&target).await?;
                if status != JobStatus::Running {
                    return Ok::<_, anyhow::Error>(status);
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .context("released diagnostic did not finish")??;
        ensure!(
            status == JobStatus::Succeeded,
            "released diagnostic did not succeed: {status:?}"
        );
        ensure!(
            std::fs::read_to_string(directory.join("target-executed"))? == "executed\n",
            "successful diagnostic did not execute its command"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    // Every assertion above returns an error so failures still clean up only
    // the units successfully created by this test and its unique directory.
    let cleanup = if owned_units.is_empty() {
        Ok(())
    } else {
        let mut args = vec!["stop".into(), "--".into()];
        args.extend(owned_units.iter().rev().cloned());
        let stopped = privileged.execute(Path::new("systemctl"), &args).await;
        for unit in &owned_units {
            let _ = privileged
                .execute(
                    Path::new("systemctl"),
                    &["reset-failed".into(), "--".into(), unit.clone()],
                )
                .await;
        }
        stopped.and_then(|output| {
            ensure!(
                output.success,
                "cannot stop owned queued fixtures: {}",
                output.stderr
            );
            Ok(())
        })
    };
    let removed = if cleanup.is_ok() {
        std::fs::remove_dir_all(&directory)
    } else {
        // Keep scripts available if systemd could not confirm their stop.
        Ok(())
    };
    result?;
    cleanup?;
    removed?;
    Ok(())
}

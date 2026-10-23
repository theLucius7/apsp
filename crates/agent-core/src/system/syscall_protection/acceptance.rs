use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

#[tokio::test]
#[ignore = "requires explicit disposable guest opt-in, Linux/root/systemd, Python 3, and no inherited seccomp filter"]
async fn real_systemd_swap_syscalls_are_denied_before_payload_and_inherited() -> Result<()> {
    ensure!(cfg!(target_os = "linux"), "requires Linux/systemd");
    ensure!(
        std::env::var("APSP_SWAP_TEST_DISPOSABLE_GUEST").as_deref() == Ok("1"),
        "swap syscall test is restricted to an explicitly authorized disposable guest"
    );
    let ops: Arc<dyn Privileged> = Arc::new(SystemOps);
    let identity = ops.execute(Path::new("id"), &["-u".into()]).await?;
    ensure!(
        identity.success && identity.stdout.trim() == "0",
        "requires root"
    );
    let services = SystemServiceManager::new(ops.clone(), ServiceBackend::Systemd);
    let directory = std::env::temp_dir().join(format!("apsp-swap-test-{}", Uuid::new_v4()));
    fs::create_dir(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    let script = directory.join("probe.py");
    fs::write(&script, include_str!("probe.py"))?;
    let mut units = Vec::new();
    let swaps_before = fs::read("/proc/swaps")?;
    let result = async {
        // Negative control proves CAP_SYS_ADMIN reaches pathname lookup without
        // using a real swap file; absence must return ENOENT rather than EPERM.
        let control = ops.execute_bounded(Path::new("/usr/bin/python3"), &[
            script.to_str().unwrap().into(), directory.to_str().unwrap().into(), "2".into(),
        ], 8, 16 * 1024).await?;
        ensure!(control.output.success && !control.timed_out && !control.truncated,
            "unfiltered nonexistent-path negative control failed: {}", control.output.stderr);
        let control_report: serde_json::Value = serde_json::from_str(&control.output.stdout)?;
        verify_report(&control_report, 2)?;

        let job: ServiceJob = serde_json::from_value(serde_json::json!({
            "unit": format!("apsp-diagnostic-{}.service", Uuid::new_v4()),
            "program": "/usr/bin/python3",
            "args": [script, directory, "1", "filtered.json"],
            "working_directory": directory,
            "timeout_secs": 15,
        }))?;
        units.push(job.unit.clone());
        services.start_job(&job).await?;
        let status = wait_finished(&services, &job.unit).await?;
        ensure!(status == JobStatus::Succeeded, "protected fixture failed: {status:?}");
        let report: serde_json::Value = serde_json::from_slice(&fs::read(directory.join("filtered.json"))?)?;
        verify_report(&report, 1)?;
        let properties = ops.execute(Path::new("systemctl"), &[
            "show".into(), "--property=NoNewPrivileges,SystemCallFilter,SystemCallErrorNumber,SystemCallArchitectures,ExecStartPre".into(),
            "--".into(), job.unit.clone(),
        ]).await?;
        ensure!(properties.success, "cannot read protected service properties");
        let text = &properties.stdout;
        assert!(text.lines().any(|line| line == "NoNewPrivileges=yes"));
        assert!(text.lines().any(|line| line == "SystemCallErrorNumber=1"));
        assert!(text.lines().any(|line| line == "SystemCallArchitectures=native"));
        let filter = text.lines().find_map(|line| line.strip_prefix("SystemCallFilter=")).unwrap();
        assert!(filter.starts_with('~'));
        let calls: Vec<_> = filter.trim_start_matches('~').split_whitespace().collect();
        assert_eq!(calls.len(), 2);
        assert!(calls.contains(&"swapon") && calls.contains(&"swapoff"));
        assert!(text.contains("/usr/bin/awk"));

        // Model a skipped installation by omitting the filter. The production
        // ExecStartPre must refuse before touching the payload marker.
        let unit = format!("apsp-diagnostic-{}.service", Uuid::new_v4());
        units.push(unit.clone());
        let output = ops.execute(Path::new("systemd-run"), &[
            format!("--unit={unit}"), "--no-block".into(), "--property=Type=oneshot".into(),
            "--property=RemainAfterExit=yes".into(), "--property=TimeoutStartSec=10s".into(),
            "--property=NoNewPrivileges=yes".into(), "--property=SystemCallArchitectures=native".into(), FILTER_CHECK.into(),
            "--".into(), "/usr/bin/touch".into(), directory.join("must-not-run").to_str().unwrap().into(),
        ]).await?;
        ensure!(output.success, "cannot submit missing-filter fixture");
        assert!(matches!(wait_finished(&services, &unit).await?, JobStatus::Failed { .. }));
        assert!(!directory.join("must-not-run").exists());
        assert_eq!(fs::read("/proc/swaps")?, swaps_before);
        assert!(!directory.join("never-created-swap").exists());
        println!("swap syscall evidence: negative_control=ENOENT protected_direct_fork_exec=EPERM skipped_filter_payload=absent swaps_unchanged=true");
        println!("{}", serde_json::json!({"control": control_report, "protected": report}));
        println!("{text}");
        Ok::<_, anyhow::Error>(())
    }.await;
    for unit in &units {
        let _ = services.stop(unit).await;
        let _ = ops
            .execute(
                Path::new("systemctl"),
                &["reset-failed".into(), "--".into(), unit.clone()],
            )
            .await;
    }
    let unchanged = fs::read("/proc/swaps")? == swaps_before;
    fs::remove_dir_all(directory)?;
    ensure!(unchanged, "swap table changed during fixture");
    result
}

fn verify_report(report: &serde_json::Value, expected: u64) -> Result<()> {
    for process in ["direct", "fork", "exec"] {
        for operation in ["swapon", "swapoff"] {
            ensure!(
                report[process]["errno"][operation].as_u64() == Some(expected),
                "unexpected syscall result"
            );
        }
    }
    Ok(())
}

async fn wait_finished(services: &SystemServiceManager, unit: &str) -> Result<JobStatus> {
    timeout(Duration::from_secs(20), async {
        loop {
            let status = services.job_status(unit).await?;
            if matches!(status, JobStatus::Succeeded | JobStatus::Failed { .. }) {
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("fixture did not finish")?
}

use super::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Observer {
    process: Mutex<Option<CommandProcessIdentity>>,
    starts: AtomicUsize,
    cancelled: AtomicBool,
    fail_registration: bool,
}

impl CommandObserver for Observer {
    fn spawned(&self, process: &CommandProcessIdentity) -> Result<()> {
        *self.process.lock().unwrap() = Some(process.clone());
        ensure!(
            !self.fail_registration,
            "injected durable registration failure"
        );
        Ok(())
    }
    fn started(&self) -> Result<()> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn cancellation_requested(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[tokio::test]
async fn cancellation_confirms_the_entire_managed_group_and_reaps_the_parent() -> Result<()> {
    let observer = Arc::new(Observer::default());
    let worker = observer.clone();
    let task = tokio::spawn(async move {
        execute(
            Path::new("/bin/sh"),
            &[
                "-c".into(),
                "printf started; trap '' TERM; (trap '' TERM; sleep 30) & wait".into(),
            ],
            30,
            1024,
            worker.as_ref(),
        )
        .await
    });
    timeout(Duration::from_secs(5), async {
        while observer.starts.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    let process = observer.process.lock().unwrap().clone().unwrap();
    // A different start identity must never cause a reused PID to be killed.
    recover(&CommandProcessIdentity {
        started: "different-boot-or-process".into(),
        ..process.clone()
    })
    .await?;
    assert!(identity(process.pid).await?.is_some());
    observer.cancelled.store(true, Ordering::SeqCst);
    let result = timeout(Duration::from_secs(8), task).await???;
    assert!(result.cancelled);
    assert!(!result.execution.output.success);
    assert!(!group_live(process.pid).await?);
    assert!(identity(process.pid).await?.is_none());
    assert_eq!(observer.starts.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_cancelled_or_unjournaled_gate_never_executes_the_payload() -> Result<()> {
    for fail_registration in [false, true] {
        let marker = std::env::temp_dir().join(format!("apsp-gate-{}", Uuid::new_v4()));
        let observer = Observer {
            fail_registration,
            ..Observer::default()
        };
        observer
            .cancelled
            .store(!fail_registration, Ordering::SeqCst);
        let result = execute(
            Path::new("/bin/sh"),
            &["-c".into(), format!("touch '{}'", marker.display())],
            5,
            1024,
            &observer,
        )
        .await;
        assert!(!marker.exists());
        assert_eq!(observer.starts.load(Ordering::SeqCst), 0);
        if fail_registration {
            assert!(result.is_err());
        } else {
            assert!(result?.cancelled);
        }
        let process = observer.process.lock().unwrap().clone().unwrap();
        assert!(!group_live(process.pid).await?);
        assert!(identity(process.pid).await?.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn timeout_and_success_cleanup_descendants_and_keep_output_bounds() -> Result<()> {
    let observer = Observer::default();
    let result = execute(
        Path::new("/bin/sh"),
        &["-c".into(), "sleep 30 & wait".into()],
        1,
        64,
        &observer,
    )
    .await?;
    assert!(result.execution.timed_out);
    assert!(!result.cancelled);
    let pid = observer.process.lock().unwrap().as_ref().unwrap().pid;
    assert!(!group_live(pid).await?);
    let observer = Observer::default();
    let result = execute(
        Path::new("/bin/sh"),
        &["-c".into(), "yes data | head -c 100000; sleep 30 &".into()],
        5,
        64,
        &observer,
    )
    .await?;
    assert!(result.execution.output.success && result.execution.truncated);
    assert_eq!(result.execution.output.stdout.len(), 64);
    let pid = observer.process.lock().unwrap().as_ref().unwrap().pid;
    assert!(!group_live(pid).await?);
    Ok(())
}

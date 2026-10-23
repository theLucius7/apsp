#![forbid(unsafe_code)]
#![cfg(unix)]
use apsp_adapter_sdk::{
    BoxFuture, CommandOutput, DiagnosticAdapter, DiagnosticSpec, Execution, Privileged,
};
use apsp_adapter_tcpquality::{AUXILIARY_FILES, TcpQualityAdapter};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
#[path = "diagnostic/fixture.rs"]
mod fixture;
#[path = "diagnostic/parameters.rs"]
mod parameters;
#[path = "diagnostic/reports.rs"]
mod reports;
use fixture::{Fixture, Recorder, chapter, report};

#[tokio::test]
async fn fixed_command_and_budget_use_only_bounded_identity_calls_and_private_files() {
    let fixture = Fixture::new(2);
    let mut spec = fixture.spec();
    spec.timeout_secs = 30;
    spec.options
        .insert("environment_section".into(), "true".into());
    spec.options.insert("count".into(), "8".into());
    spec.options.insert("concurrency".into(), "2".into());
    let privileged = Recorder::default();
    let adapter = TcpQualityAdapter::new();
    assert_eq!(
        adapter.capabilities(),
        vec![apsp_adapter_tcpquality::CAPABILITY]
    );
    assert_eq!(adapter.describe().plugin_name, "tcpquality");
    assert_eq!(adapter.describe().binary_name, "apsp-tcp-probe");
    assert_eq!(
        adapter.auxiliary_files(),
        AUXILIARY_FILES.map(str::to_owned)
    );
    let job = adapter.prepare(&spec, &privileged).await.unwrap();
    assert_eq!(job.unit, format!("apsp-diagnostic-{}.service", spec.id));
    assert_eq!(job.program, spec.binary_path);
    assert_eq!(job.working_directory, spec.job_dir);
    assert_eq!(job.timeout_secs, 30);
    assert_eq!(job.memory_max.get(), 64 * 1024 * 1024);
    assert_eq!(job.tasks_max.get(), 32);
    assert_eq!(job.cpu_weight.get(), 10);
    assert_eq!(job.io_weight.get(), 10);
    assert_eq!(job.oom_score_adjust.get(), 500);
    assert_eq!(
        job.args,
        vec![
            "--workspace",
            spec.job_dir.to_str().unwrap(),
            "--targets",
            "targets.json",
            "--target-digest",
            &spec.options["target_digest"],
            "--ip-version",
            "4",
            "--count",
            "8",
            "--concurrency",
            "2",
            "--no-rank-upload"
        ]
    );
    assert_eq!(
        *privileged.probes.lock().unwrap(),
        vec!["--version", "--build-info"]
    );
    assert_eq!(*privileged.created.lock().unwrap(), vec![0o700]);
    assert_eq!(*privileged.written.lock().unwrap(), vec![0o600]);
    assert_eq!(
        std::fs::read_to_string(spec.job_dir.join("targets.json")).unwrap(),
        spec.options["targets"]
    );
    assert_eq!(job.args.last().unwrap(), "--no-rank-upload");
    let fresh = Fixture::new(1);
    let mut longer = fresh.spec();
    longer.timeout_secs = 1800;
    assert_eq!(
        adapter
            .prepare(&longer, &Recorder::default())
            .await
            .unwrap()
            .timeout_secs,
        60
    );
}

#[tokio::test]
async fn existing_output_and_changed_frozen_input_prevent_duplicate_execution() {
    let fixture = Fixture::new(1);
    let spec = fixture.spec();
    fixture.workspace();
    fixture.write("result.json", b"saved partial report");
    assert!(
        TcpQualityAdapter::new()
            .prepare(&spec, &Recorder::default())
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(spec.job_dir.join("result.json")).unwrap(),
        b"saved partial report"
    );
    std::fs::remove_file(spec.job_dir.join("result.json")).unwrap();
    fixture.write("targets.json", b"old independent scope");
    assert!(
        TcpQualityAdapter::new()
            .prepare(&spec, &Recorder::default())
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(spec.job_dir.join("targets.json")).unwrap(),
        b"old independent scope"
    );
}

#[tokio::test]
async fn preparation_timeout_does_not_create_workspace_or_start_any_service() {
    let fixture = Fixture::new(1);
    let mut spec = fixture.spec();
    spec.timeout_secs = 1;
    let privileged = Recorder {
        hang: true,
        ..Default::default()
    };
    let started = tokio::time::Instant::now();
    assert!(
        TcpQualityAdapter::new()
            .prepare(&spec, &privileged)
            .await
            .is_err()
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(!spec.job_dir.exists());
    assert!(privileged.created.lock().unwrap().is_empty());
}

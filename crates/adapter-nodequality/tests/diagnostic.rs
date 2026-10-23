#![forbid(unsafe_code)]

use apsp_adapter_nodequality::{
    MAX_REPORT_BYTES, NodeQualityAdapter, OFFLINE_ROOTFS_VERSION, VERSION,
};
use apsp_adapter_sdk::{BoxFuture, CommandOutput, DiagnosticAdapter, DiagnosticSpec, Privileged};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "apsp-nodequality-adapter-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn spec(&self) -> DiagnosticSpec {
        DiagnosticSpec {
            id: "12345678-1234-1234-1234-123456789abc".into(),
            version: VERSION.into(),
            binary_path: self.0.join("nodequality"),
            job_dir: self.0.join("job"),
            timeout_secs: 1800,
            options: BTreeMap::new(),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[derive(Default)]
struct FakePrivileged {
    calls: Mutex<Vec<Vec<String>>>,
    invalid_version: bool,
    artifact_version: Option<String>,
}

impl Privileged for FakePrivileged {
    fn execute<'a>(&'a self, _: &'a Path, args: &'a [String]) -> BoxFuture<'a, CommandOutput> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(args.to_vec());
            Ok(CommandOutput {
                success: true,
                stdout: if self.invalid_version {
                    "another program".into()
                } else {
                    format!(
                        "nodequality {}\n",
                        self.artifact_version.as_deref().unwrap_or(VERSION)
                    )
                },
                stderr: String::new(),
            })
        })
    }

    fn create_dir<'a>(
        &'a self,
        path: &'a Path,
        mode: u32,
        _: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            assert_eq!(mode, 0o700);
            tokio::fs::create_dir_all(path).await?;
            Ok(())
        })
    }

    fn write_file<'a>(
        &'a self,
        path: &'a Path,
        bytes: &'a [u8],
        mode: u32,
        _: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            assert!(matches!(
                path.file_name().unwrap().to_str(),
                Some("daily-targets.json" | "node-ips.json")
            ));
            assert_eq!(mode, 0o600);
            tokio::fs::write(path, bytes).await?;
            Ok(())
        })
    }

    fn atomic_symlink<'a>(&'a self, _: &'a Path, _: &'a Path) -> BoxFuture<'a, ()> {
        Box::pin(async { anyhow::bail!("unexpected symlink") })
    }

    fn remove_symlink<'a>(&'a self, _: &'a Path) -> BoxFuture<'a, ()> {
        Box::pin(async { anyhow::bail!("unexpected symlink removal") })
    }

    fn install_archive<'a>(&'a self, _: &'a Path, _: &'a Path, _: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async { anyhow::bail!("unexpected archive install") })
    }
}

#[tokio::test]
async fn all_full_versions_are_denied_before_executing_or_creating_anything() {
    let scratch = Scratch::new();
    let privileged = FakePrivileged::default();
    for version in [
        VERSION,
        OFFLINE_ROOTFS_VERSION,
        "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r17",
        "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r18",
        "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r5",
        "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r4",
        "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r2",
        "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r3",
    ] {
        for upload in ["true", "false"] {
            let mut spec = scratch.spec();
            spec.version = version.into();
            spec.options.insert("upload_report".into(), upload.into());
            let error = NodeQualityAdapter::new()
                .prepare(&spec, &privileged)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("离线受控工具链"));
            assert!(!spec.job_dir.exists());
            assert!(privileged.calls.lock().unwrap().is_empty());
        }
    }
    let mut explicit = scratch.spec();
    explicit.options.insert("mode".into(), "full".into());
    assert!(
        NodeQualityAdapter::new()
            .prepare(&explicit, &privileged)
            .await
            .is_err()
    );
    assert!(!explicit.job_dir.exists());
}

#[tokio::test]
async fn daily_profile_is_bounded_and_persists_only_whitelisted_targets() {
    let scratch = Scratch::new();
    let privileged = FakePrivileged::default();
    let mut spec = scratch.spec();
    spec.timeout_secs = 90;
    spec.options = BTreeMap::from([
        ("mode".into(), "daily".into()),
        (
            "daily_targets".into(),
            r#"[{"name":"private","target":"127.0.0.1","port":443}]"#.into(),
        ),
        ("environment_section".into(), "true".into()),
    ]);
    let adapter = NodeQualityAdapter::new();
    assert_eq!(
        adapter.capabilities(),
        vec![
            apsp_adapter_nodequality::MODES_CAPABILITY,
            apsp_adapter_nodequality::FULL_START_GATE_CAPABILITY,
            apsp_adapter_nodequality::NODE_QUERY_CAPABILITY
        ]
    );
    let service = adapter.prepare(&spec, &privileged).await.unwrap();
    assert_eq!(service.memory_max.get(), 64 * 1024 * 1024);
    assert_eq!(service.tasks_max.get(), 32);
    assert_eq!(service.timeout_secs, 90);
    assert!(
        service
            .args
            .windows(2)
            .any(|args| args == ["--mode", "daily"])
    );
    assert_eq!(
        std::fs::read_to_string(spec.job_dir.join("daily-targets.json")).unwrap(),
        spec.options["daily_targets"]
    );
    for (key, value) in [
        ("network_mode", "normal"),
        ("upload_report", "true"),
        (
            "daily_targets",
            r#"[{"name":"bad","target":"$(id)","port":443}]"#,
        ),
    ] {
        let mut invalid = spec.clone();
        invalid.options.insert(key.into(), value.into());
        assert!(adapter.prepare(&invalid, &privileged).await.is_err());
    }
    let mut legacy = spec.clone();
    legacy.version = "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r3".into();
    assert!(adapter.prepare(&legacy, &privileged).await.is_err());
}

#[tokio::test]
async fn legacy_signed_versions_still_collect_saved_reports_without_preparing_again() {
    let scratch = Scratch::new();
    let mut spec = scratch.spec();
    std::fs::create_dir_all(&spec.job_dir).unwrap();
    std::fs::write(spec.job_dir.join("result.txt"), "saved before restart").unwrap();
    for version in [
        "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r2",
        "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r3",
        VERSION,
    ] {
        spec.version = version.into();
        let saved = NodeQualityAdapter::new()
            .collect(&spec)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.text, "saved before restart");
        assert!(
            NodeQualityAdapter::new()
                .collect_sections(&spec)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn prepare_rejects_unpinned_versions_unknown_options_and_expansion() {
    let scratch = Scratch::new();
    let privileged = FakePrivileged::default();
    for case in 0..12 {
        let mut spec = scratch.spec();
        match case {
            0 => spec.version = "latest".into(),
            1 => {
                spec.options.insert("command".into(), "id".into());
            }
            2 => {
                spec.options.insert("ip_version".into(), "$(id)".into());
            }
            3 => spec.job_dir = scratch.0.join("%n"),
            4 => spec.id = "bad/service".into(),
            5 => spec.timeout_secs = 3601,
            6 => spec.job_dir = PathBuf::from("/"),
            7 => spec.job_dir = scratch.0.join("space path"),
            8 => spec.job_dir = scratch.0.join("wild*card"),
            9 => spec.job_dir = scratch.0.join("question?mark"),
            10 => {
                spec.options.insert("upload_report".into(), "yes".into());
            }
            11 => spec.version = "a92fca6c0067df29ddd03fdc2fee6f3000f64545".into(),
            _ => unreachable!(),
        }
        assert!(
            NodeQualityAdapter::new()
                .prepare(&spec, &privileged)
                .await
                .is_err()
        );
    }
    assert!(privileged.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn prepare_rejects_an_artifact_that_does_not_match_its_version() {
    let scratch = Scratch::new();
    let privileged = FakePrivileged {
        invalid_version: true,
        ..Default::default()
    };
    let mut spec = scratch.spec();
    spec.options = BTreeMap::from([
        ("mode".into(), "daily".into()),
        ("daily_targets".into(), "[]".into()),
    ]);
    assert!(
        NodeQualityAdapter::new()
            .prepare(&spec, &privileged)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn collect_preserves_local_report_when_online_upload_failed() {
    let scratch = Scratch::new();
    let spec = scratch.spec();
    std::fs::create_dir(&spec.job_dir).unwrap();
    std::fs::write(
        spec.job_dir.join("result.txt"),
        "硬件质量\nIP 质量\nHTTP 状态：403\n",
    )
    .unwrap();
    let output = NodeQualityAdapter::new()
        .collect(&spec)
        .await
        .unwrap()
        .unwrap();
    assert!(output.text.contains("IP 质量"));
    assert_eq!(output.report_url, None);
    std::fs::write(
        spec.job_dir.join("report-url.txt"),
        "https://nodequality.com/r/report_TOKEN-123\n",
    )
    .unwrap();
    assert_eq!(
        NodeQualityAdapter::new()
            .collect(&spec)
            .await
            .unwrap()
            .unwrap()
            .report_url,
        Some("https://nodequality.com/r/report_TOKEN-123".into())
    );
}

#[tokio::test]
async fn collect_rejects_a_remote_or_malformed_url_and_oversized_report() {
    let scratch = Scratch::new();
    let spec = scratch.spec();
    std::fs::create_dir(&spec.job_dir).unwrap();
    std::fs::write(spec.job_dir.join("result.txt"), "actual local report").unwrap();
    for value in [
        "https://evil.example/r/a",
        "https://nodequality.com/r/a?next=x",
        "https://nodequality.com/r/",
    ] {
        std::fs::write(spec.job_dir.join("report-url.txt"), value).unwrap();
        assert!(NodeQualityAdapter::new().collect(&spec).await.is_err());
    }
    std::fs::remove_file(spec.job_dir.join("report-url.txt")).unwrap();
    std::fs::write(
        spec.job_dir.join("result.txt"),
        vec![b'a'; MAX_REPORT_BYTES as usize + 1],
    )
    .unwrap();
    assert!(NodeQualityAdapter::new().collect(&spec).await.is_err());
}

#[tokio::test]
async fn collect_never_treats_a_log_or_stale_url_as_a_completed_report() {
    let scratch = Scratch::new();
    let spec = scratch.spec();
    assert!(
        NodeQualityAdapter::new()
            .collect(&spec)
            .await
            .unwrap()
            .is_none()
    );
    std::fs::create_dir(&spec.job_dir).unwrap();
    std::fs::write(spec.job_dir.join("log.txt"), "mount failed").unwrap();
    std::fs::write(
        spec.job_dir.join("report-url.txt"),
        "https://nodequality.com/r/stale",
    )
    .unwrap();
    assert!(
        NodeQualityAdapter::new()
            .collect(&spec)
            .await
            .unwrap()
            .is_none()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn collect_does_not_follow_output_symlinks() {
    let scratch = Scratch::new();
    let spec = scratch.spec();
    std::fs::create_dir(&spec.job_dir).unwrap();
    let secret = scratch.0.join("private-file");
    std::fs::write(&secret, "private").unwrap();
    std::os::unix::fs::symlink(secret, spec.job_dir.join("result.txt")).unwrap();
    assert!(NodeQualityAdapter::new().collect(&spec).await.is_err());
}

#[tokio::test]
async fn chapters_remain_readable_when_the_final_report_is_missing_or_another_chapter_is_bad() {
    let scratch = Scratch::new();
    let spec = scratch.spec();
    std::fs::create_dir(&spec.job_dir).unwrap();
    let chapter = serde_json::json!({"name":"header_info","text":"saved header","complete":true,"revision":2,"collected_at":1700000000});
    std::fs::write(
        spec.job_dir.join("section-header_info.json"),
        serde_json::to_vec(&chapter).unwrap(),
    )
    .unwrap();
    std::fs::write(
        spec.job_dir.join("section-hardware_quality.json"),
        "invalid JSON",
    )
    .unwrap();
    let adapter = NodeQualityAdapter::new();
    assert!(adapter.collect(&spec).await.unwrap().is_none());
    let chapters = adapter.collect_sections(&spec).await.unwrap();
    assert_eq!(chapters.len(), 1);
    assert_eq!(chapters[0].text, "saved header");
    assert!(chapters[0].complete);
    for revision in ["r2", "r3"] {
        let mut legacy = spec.clone();
        legacy.version = format!("a92fca6c0067df29ddd03fdc2fee6f3000f64545-{revision}");
        std::fs::write(legacy.job_dir.join("result.txt"), "unchanged old report").unwrap();
        assert_eq!(
            adapter.collect(&legacy).await.unwrap().unwrap().text,
            "unchanged old report"
        );
        let chapters = adapter.collect_sections(&legacy).await.unwrap();
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].text, "saved header");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn chapter_collection_never_follows_symlinks_and_rejects_mismatched_names() {
    let scratch = Scratch::new();
    let spec = scratch.spec();
    std::fs::create_dir(&spec.job_dir).unwrap();
    let secret = scratch.0.join("private-file");
    std::fs::write(&secret, serde_json::to_vec(&serde_json::json!({"name":"header_info","text":"private","complete":true,"revision":1,"collected_at":1700000000})).unwrap()).unwrap();
    std::os::unix::fs::symlink(secret, spec.job_dir.join("section-header_info.json")).unwrap();
    std::fs::write(spec.job_dir.join("section-ip_quality.json"), serde_json::to_vec(&serde_json::json!({"name":"hardware_quality","text":"wrong chapter","complete":true,"revision":1,"collected_at":1700000000})).unwrap()).unwrap();
    assert!(
        NodeQualityAdapter::new()
            .collect_sections(&spec)
            .await
            .unwrap()
            .is_empty()
    );
}

#[path = "diagnostic/versions.rs"]
mod versions;

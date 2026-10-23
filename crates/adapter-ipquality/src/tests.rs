use super::*;
use serde_json::json;
#[cfg(unix)]
use std::{
    fs as stdfs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

fn spec() -> DiagnosticSpec {
    let id = "00000000-0000-4000-8000-000000000001";
    DiagnosticSpec {
        id: id.into(),
        version: VERSION.into(),
        binary_path: Path::new("/tmp/ipquality-fixture")
            .join(VERSION)
            .join(BINARY),
        job_dir: Path::new("/tmp/ipquality-fixture").join(id),
        timeout_secs: 120,
        options: BTreeMap::from([
            ("ip_version".into(), "4".into()),
            ("environment_section".into(), "true".into()),
        ]),
    }
}

fn context(spec: &DiagnosticSpec) -> ExecutionContext {
    ExecutionContext {
        schema: 1,
        job_id: spec.id.clone(),
        version: VERSION.into(),
        ip_version: spec.options["ip_version"].clone(),
        artifact_sha256: "1".repeat(64),
    }
}

fn report(spec: &DiagnosticSpec, finished: bool) -> Value {
    let at = files::now().unwrap().saturating_sub(2);
    json!({
        "schema":1,"plugin":"ipquality","version":VERSION,"job_id":spec.id,"ip_version":spec.options["ip_version"],
        "artifact_sha256":"1".repeat(64),"source_commit":SOURCE_COMMIT,"source_sha256":SOURCE_SHA256,
        "started_at":at,"finished_at":finished.then_some(at+1),"egress_ip":null,"upstream":null,
        "attempts":[{"seq":1,"provider":"egress-discovery","dataset":"egress","target_ip":null,
            "url":"https://api64.ipify.org/","status":"failed","attempted_at":at,"elapsed_ms":10,
            "http_status":null,"curl_exit":28,"response_bytes":0,"error_kind":"timeout","error_message":"请求超时"}]
    })
}

#[test]
fn parameter_whitelist_rejects_expansion_custom_commands_and_unknown_versions() {
    assert_eq!(validate(&spec()).unwrap(), "4");
    for option in [
        "command",
        "upload_report",
        "proxy",
        "user_agent",
        "allow_speedtest_staged",
    ] {
        let mut input = spec();
        input.options.insert(option.into(), "true".into());
        assert!(validate(&input).is_err(), "{option}");
    }
    for value in ["both", "ipv4", "7", "4;echo"] {
        let mut input = spec();
        input.options.insert("ip_version".into(), value.into());
        assert!(validate(&input).is_err());
    }
    let mut input = spec();
    input.timeout_secs = 301;
    assert!(validate(&input).is_err());
    input = spec();
    input.version = "main".into();
    assert!(validate(&input).is_err());
    input = spec();
    input.job_dir = Path::new("/tmp/$HOME").join(&input.id);
    assert!(validate(&input).is_err());
    input = spec();
    input.job_dir = Path::new("/tmp/private/../other").join(&input.id);
    assert!(validate(&input).is_err());
    input = spec();
    input.id = "not-a-uuid".into();
    assert!(validate(&input).is_err());
}

#[test]
fn partial_and_failed_discovery_remain_typed_unknown_and_are_bound_to_the_job() {
    let input = spec();
    assert!(
        report::Report::parse(
            &report(&input, false).to_string(),
            &context(&input),
            input.timeout_secs
        )
        .unwrap()
        .finished_at
        .is_none()
    );
    assert!(
        report::Report::parse(
            &report(&input, true).to_string(),
            &context(&input),
            input.timeout_secs
        )
        .unwrap()
        .finished_at
        .is_some()
    );
    for key in [
        "schema",
        "plugin",
        "version",
        "job_id",
        "ip_version",
        "artifact_sha256",
        "source_commit",
        "source_sha256",
    ] {
        let mut output = report(&input, true);
        output[key] = json!("untrusted");
        assert!(
            report::Report::parse(&output.to_string(), &context(&input), input.timeout_secs)
                .is_err(),
            "{key}"
        );
    }
    for key in ["finished_at", "egress_ip", "upstream"] {
        let mut output = report(&input, true);
        output.as_object_mut().unwrap().remove(key);
        assert!(
            report::Report::parse(&output.to_string(), &context(&input), input.timeout_secs)
                .is_err(),
            "missing {key}"
        );
    }
    let mut output = report(&input, true);
    output["upstream"] = json!({"Head":{"IP":"192.0.2.1","Version":"v2026-09-16"}});
    assert!(
        report::Report::parse(&output.to_string(), &context(&input), input.timeout_secs).is_err()
    );
}

#[test]
fn misleading_success_unbounded_requests_and_misclassified_denials_are_rejected() {
    let input = spec();
    for (key, value) in [
        ("status", json!("succeeded")),
        ("seq", json!(2)),
        ("provider", json!("seven-independent-libraries")),
        ("dataset", json!("IPv4")),
        ("url", json!("https://example.test/query")),
        ("url", json!("http://api64.ipify.org/")),
        ("url", json!("https://secret@api64.ipify.org/")),
        ("elapsed_ms", json!(300001)),
        ("response_bytes", json!(2 * 1024 * 1024 + 1)),
        ("http_status", json!(403)),
        ("http_status", json!(429)),
        ("curl_exit", json!(256)),
        ("error_kind", json!("clean")),
        ("error_message", json!("")),
        ("error_message", json!("unsafe\nmessage")),
        ("target_ip", json!("192.0.2.1")),
    ] {
        let mut output = report(&input, true);
        output["attempts"][0][key] = value;
        assert!(
            report::Report::parse(&output.to_string(), &context(&input), input.timeout_secs)
                .is_err(),
            "{key}"
        );
    }
    for status in [403, 429] {
        let mut output = report(&input, true);
        output["attempts"][0]["http_status"] = json!(status);
        output["attempts"][0]["error_kind"] = json!(if status == 403 {
            "http_403"
        } else {
            "http_429"
        });
        assert!(
            report::Report::parse(&output.to_string(), &context(&input), input.timeout_secs)
                .is_ok()
        );
    }
    let mut output = report(&input, true);
    output["attempts"] = json!(
        (0..65)
            .map(|_| output["attempts"][0].clone())
            .collect::<Vec<_>>()
    );
    assert!(
        report::Report::parse(&output.to_string(), &context(&input), input.timeout_secs).is_err()
    );
    let mut output = report(&input, true);
    let mut duplicate = output["attempts"][0].clone();
    duplicate["seq"] = json!(2);
    output["attempts"].as_array_mut().unwrap().push(duplicate);
    assert!(
        report::Report::parse(&output.to_string(), &context(&input), input.timeout_secs).is_err()
    );
}

#[test]
fn disabled_services_must_have_explicit_unknown_receipts_without_network_success() {
    let input = spec();
    let mut output = report(&input, true);
    output["attempts"] = json!([{"seq":1,"provider":"dnsbl-disabled","dataset":"DNSBL","target_ip":null,"url":null,
        "status":"not_attempted","attempted_at":null,"elapsed_ms":null,"http_status":null,"curl_exit":null,"response_bytes":null,
        "error_kind":"not_attempted","error_message":"未授权，结果未知"}]);
    assert!(
        report::Report::parse(&output.to_string(), &context(&input), input.timeout_secs).is_ok()
    );
    output["attempts"][0]["http_status"] = json!(200);
    assert!(
        report::Report::parse(&output.to_string(), &context(&input), input.timeout_secs).is_err()
    );
}

#[test]
fn partial_section_cannot_precede_its_observed_request_or_claim_completion() {
    let input = spec();
    let mut output = report(&input, false);
    let at = files::now().unwrap().saturating_sub(2);
    output["started_at"] = json!(at);
    output["attempts"][0]["attempted_at"] = json!(at + 1);
    let parsed =
        report::Report::parse(&output.to_string(), &context(&input), input.timeout_secs).unwrap();
    assert!(parsed.validate_section_time(at, false).is_err());
    assert!(parsed.validate_section_time(at + 1, false).is_ok());
    assert!(parsed.validate_section_time(at + 1, true).is_err());
}

#[cfg(unix)]
struct Fixture {
    root: PathBuf,
}
#[cfg(unix)]
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = stdfs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "apsp-ip-adapter-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
        stdfs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let directory = root.join(VERSION);
        stdfs::create_dir(&directory).unwrap();
        Self::write_path(
            &directory.join(BINARY),
            b"TEST ONLY fixture; never executed",
            0o755,
        );
        let arch = native_arch().unwrap();
        let auxiliary: BTreeMap<_, _> = AUXILIARY_FILES.iter().map(|name| (*name, json!({"sha256":match *name {"rootfs-manifest.json"=>"2".repeat(64),"source.tar.gz"=>"3".repeat(64),_=>"1".repeat(64)},"size":1}))).collect();
        Self::write_path(&directory.join("release.json"), json!({"artifacts":[{"name":"ipquality","version":VERSION,"arch":arch,"binary_name":"ipquality","format":"tar.gz","auxiliary_files":auxiliary}]}).to_string().as_bytes(), 0o644);
        Self::write_path(
            &directory.join("SHA256SUMS"),
            format!("{}  ipquality/{VERSION}/{arch}\n", "1".repeat(64)).as_bytes(),
            0o644,
        );
        Self::write_path(&directory.join("build-info.json"), json!({
            "schema":1,"plugin":"ipquality","version":VERSION,"arch":arch,"profile":"ipquality-node-v1",
            "source_commit":SOURCE_COMMIT,"source_sha256":SOURCE_SHA256,"source_lock_sha256":"1".repeat(64),
            "policy_sha256":"1".repeat(64),"transport_sha256":"1".repeat(64),"rootfs_sha256":"1".repeat(64),
            "rootfs_manifest_sha256":"2".repeat(64),"license_review_sha256":"1".repeat(64),"source_archive_sha256":"3".repeat(64),"factory_provenance_sha256":"1".repeat(64)
        }).to_string().as_bytes(), 0o644);
        Self { root }
    }
    fn spec(&self) -> DiagnosticSpec {
        let mut spec = spec();
        spec.binary_path = self.root.join(VERSION).join(BINARY);
        spec.job_dir = self.root.join(&spec.id);
        spec
    }
    fn write_path(path: &Path, bytes: &[u8], mode: u32) {
        let mut file = stdfs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(mode)
            .open(path)
            .unwrap();
        file.write_all(bytes).unwrap();
        stdfs::set_permissions(path, stdfs::Permissions::from_mode(mode)).unwrap();
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        Self::write_path(&self.spec().job_dir.join(name), bytes, 0o600);
    }
}
#[cfg(unix)]
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = stdfs::remove_dir_all(&self.root);
    }
}

#[cfg(unix)]
#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<String>>,
    wrong_version: bool,
    hang: bool,
}
#[cfg(unix)]
impl Privileged for Recorder {
    fn execute<'a>(
        &'a self,
        _: &'a Path,
        _: &'a [String],
    ) -> BoxFuture<'a, apsp_adapter_sdk::CommandOutput> {
        Box::pin(async { anyhow::bail!("unbounded execute is forbidden in this fixture") })
    }
    fn execute_bounded<'a>(
        &'a self,
        _: &'a Path,
        args: &'a [String],
        seconds: u32,
        maximum: usize,
    ) -> BoxFuture<'a, apsp_adapter_sdk::Execution> {
        Box::pin(async move {
            assert_eq!(args, &["--version".to_owned()]);
            assert_eq!(seconds, 2);
            assert_eq!(maximum, 4096);
            self.calls.lock().unwrap().push(args[0].clone());
            if self.hang {
                std::future::pending::<()>().await;
            }
            Ok(apsp_adapter_sdk::Execution {
                output: apsp_adapter_sdk::CommandOutput {
                    success: true,
                    stdout: if self.wrong_version {
                        "ipquality main".into()
                    } else {
                        format!("ipquality {VERSION}\n")
                    },
                    stderr: String::new(),
                },
                timed_out: false,
                truncated: false,
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
            stdfs::create_dir_all(path)?;
            stdfs::set_permissions(path, stdfs::Permissions::from_mode(mode))?;
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
            Fixture::write_path(path, bytes, mode);
            Ok(())
        })
    }
    fn atomic_symlink<'a>(&'a self, _: &'a Path, _: &'a Path) -> BoxFuture<'a, ()> {
        Box::pin(async { anyhow::bail!("unexpected symlink") })
    }
    fn remove_symlink<'a>(&'a self, _: &'a Path) -> BoxFuture<'a, ()> {
        Box::pin(async { anyhow::bail!("unexpected removal") })
    }
    fn install_archive<'a>(&'a self, _: &'a Path, _: &'a Path, _: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async { anyhow::bail!("unexpected installation") })
    }
}

#[cfg(unix)]
#[tokio::test]
async fn command_is_bound_to_actual_signed_archive_hash_and_preserves_core_timeout_budget() {
    let fixture = Fixture::new();
    let spec = fixture.spec();
    let recorder = Recorder::default();
    let adapter = IpQualityAdapter::new();
    let service = adapter.prepare(&spec, &recorder).await.unwrap();
    assert_eq!(service.unit, format!("apsp-diagnostic-{}.service", spec.id));
    assert_eq!(service.timeout_secs, spec.timeout_secs);
    assert_eq!(service.memory_max.get(), 128 * 1024 * 1024);
    assert_eq!(service.tasks_max.get(), 64);
    assert_eq!(service.cpu_weight.get(), 10);
    assert_eq!(service.io_weight.get(), 10);
    assert_eq!(service.oom_score_adjust.get(), 500);
    assert_eq!(
        service.args,
        vec![
            "--workspace".to_owned(),
            spec.job_dir.to_str().unwrap().into(),
            "--job-id".into(),
            spec.id.clone(),
            "--ip-version".into(),
            "4".into(),
            "--artifact-sha256".into(),
            "1".repeat(64)
        ]
    );
    assert_eq!(*recorder.calls.lock().unwrap(), vec!["--version"]);
    assert_eq!(adapter.auxiliary_files().len(), 6);
    assert_eq!(adapter.capabilities(), vec![CAPABILITY]);
    assert!(adapter.prepare(&spec, &recorder).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn signed_profile_mismatch_and_uncertain_identity_never_create_a_workspace() {
    for key in [
        "source_commit",
        "profile",
        "arch",
        "rootfs_sha256",
        "source_archive_sha256",
    ] {
        let fixture = Fixture::new();
        let spec = fixture.spec();
        let path = spec.binary_path.parent().unwrap().join("build-info.json");
        let mut build: Value =
            serde_json::from_str(&stdfs::read_to_string(&path).unwrap()).unwrap();
        build[key] = json!("untrusted");
        Fixture::write_path(&path, build.to_string().as_bytes(), 0o644);
        let recorder = Recorder::default();
        assert!(
            IpQualityAdapter::new()
                .prepare(&spec, &recorder)
                .await
                .is_err()
        );
        assert!(!spec.job_dir.exists());
        assert!(recorder.calls.lock().unwrap().is_empty());
    }
    for recorder in [
        Recorder {
            wrong_version: true,
            ..Default::default()
        },
        Recorder {
            hang: true,
            ..Default::default()
        },
    ] {
        let fixture = Fixture::new();
        let spec = fixture.spec();
        assert!(
            IpQualityAdapter::new()
                .prepare(&spec, &recorder)
                .await
                .is_err()
        );
        assert!(!spec.job_dir.exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn partial_json_and_complete_unknown_results_survive_binary_removal_without_rerunning() {
    let fixture = Fixture::new();
    let spec = fixture.spec();
    let adapter = IpQualityAdapter::new();
    adapter.prepare(&spec, &Recorder::default()).await.unwrap();
    for finished in [false, true] {
        let output = report(&spec, finished).to_string();
        fixture.write("result.json", output.as_bytes());
        fixture.write(
            "section-ipquality_result.json",
            serde_json::to_string(&DiagnosticSection {
                name: "ipquality_result".into(),
                text: output.clone(),
                complete: finished,
                revision: if finished { 2 } else { 1 },
                collected_at: files::now().unwrap(),
            })
            .unwrap()
            .as_bytes(),
        );
        assert_eq!(adapter.collect(&spec).await.unwrap().unwrap().text, output);
        assert_eq!(
            adapter.collect_sections(&spec).await.unwrap()[0].complete,
            finished
        );
    }
    fixture.write(
        "section-ipquality_result.json",
        serde_json::to_string(&DiagnosticSection {
            name: "ipquality_result".into(),
            text: report(&spec, false).to_string(),
            complete: true,
            revision: 3,
            collected_at: files::now().unwrap(),
        })
        .unwrap()
        .as_bytes(),
    );
    assert!(adapter.collect_sections(&spec).await.is_err());
    fixture.write(
        "section-ipquality_result.json",
        serde_json::to_string(&DiagnosticSection {
            name: "ipquality_result".into(),
            text: report(&spec, true).to_string(),
            complete: true,
            revision: 4,
            collected_at: files::now().unwrap(),
        })
        .unwrap()
        .as_bytes(),
    );
    stdfs::remove_file(&spec.binary_path).unwrap();
    assert!(adapter.collect(&spec).await.unwrap().is_some());
    assert_eq!(adapter.collect_sections(&spec).await.unwrap().len(), 1);
    let original = stdfs::read(spec.job_dir.join("result.json")).unwrap();
    assert!(adapter.prepare(&spec, &Recorder::default()).await.is_err());
    assert_eq!(
        stdfs::read(spec.job_dir.join("result.json")).unwrap(),
        original
    );
}

#[cfg(unix)]
#[tokio::test]
async fn output_identity_permissions_symlinks_hardlinks_and_size_are_enforced() {
    let fixture = Fixture::new();
    let spec = fixture.spec();
    let adapter = IpQualityAdapter::new();
    adapter.prepare(&spec, &Recorder::default()).await.unwrap();
    let path = spec.job_dir.join("result.json");
    let mut output = report(&spec, true);
    output["artifact_sha256"] = json!("2".repeat(64));
    fixture.write("result.json", output.to_string().as_bytes());
    assert!(adapter.collect(&spec).await.is_err());
    fixture.write("result.json", report(&spec, true).to_string().as_bytes());
    stdfs::set_permissions(&path, stdfs::Permissions::from_mode(0o644)).unwrap();
    assert!(adapter.collect(&spec).await.is_err());
    stdfs::remove_file(&path).unwrap();
    fixture.write("unrelated.json", report(&spec, true).to_string().as_bytes());
    std::os::unix::fs::symlink(spec.job_dir.join("unrelated.json"), &path).unwrap();
    assert!(adapter.collect(&spec).await.is_err());
    stdfs::remove_file(&path).unwrap();
    stdfs::hard_link(spec.job_dir.join("unrelated.json"), &path).unwrap();
    assert!(adapter.collect(&spec).await.is_err());
    stdfs::remove_file(&path).unwrap();
    fixture.write("result.json", &vec![b'x'; OUTPUT_LIMIT + 1]);
    assert!(adapter.collect(&spec).await.is_err());
}

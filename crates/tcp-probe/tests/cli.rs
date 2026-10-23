#![forbid(unsafe_code)]

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    directory: PathBuf,
    digest: String,
    accepted: Arc<AtomicUsize>,
    server: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new() -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let observed = accepted.clone();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(4);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        assert_eq!(
                            stream.read(&mut [0; 1]).unwrap(),
                            0,
                            "no application payload"
                        );
                        observed.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => panic!("fixture listener: {error}"),
                }
            }
        });
        let directory = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "apsp-tcp-cli-test-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::SeqCst)
            ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory).unwrap();
        let bytes = serde_json::to_vec(&json!({"schema":1,"targets":[{"id":"00000000-0000-4000-8000-000000000001","name":"CLI loopback fixture","target":"127.0.0.1","port":address.port(),"carrier":"configured","region":null}]})).unwrap();
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(directory.join("targets.json"))
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        Self {
            directory,
            digest: format!("{:x}", Sha256::digest(&bytes)),
            accepted,
            server: Some(server),
        }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_apsp-tcp-probe"));
        command.args([
            "--workspace",
            self.directory.to_str().unwrap(),
            "--targets",
            "targets.json",
            "--target-digest",
            &self.digest,
            "--ip-version",
            "4",
            "--no-rank-upload",
        ]);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
    fn read(&self) -> Option<Value> {
        serde_json::from_slice(&fs::read(self.directory.join("result.json")).ok()?).ok()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.join().unwrap();
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn finish(mut child: Child) -> std::process::Output {
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("fixture process exceeded deadline");
        }
        thread::sleep(Duration::from_millis(5));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn actual_cli_enforces_contract_and_emits_bounded_local_json_only() {
    let fixture = Fixture::new();
    let output = finish(fixture.command().spawn().unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.len() <= apsp_tcp_probe::OUTPUT_LIMIT + 1);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(Some(report.clone()), fixture.read());
    assert_eq!(report["complete"], true);
    assert_eq!(report["target_digest"], fixture.digest);
    assert_eq!(
        report["engine"]["source_commit"],
        option_env!("APSP_NATIVE_TCP_SOURCE_COMMIT")
            .map(Value::from)
            .unwrap_or(Value::Null)
    );
    assert_eq!(
        report["targets"][0]["summary"]["connection_success_percent"],
        100.0
    );
    assert_eq!(report["parameters"]["count"], 4);
    assert_eq!(report["parameters"]["concurrency"], 1);
    assert_eq!(report["parameters"]["total_timeout_ms"], 60_000);
    assert_eq!(report["upload_enabled"], false);
    assert_eq!(report["ranking_enabled"], false);
    assert_eq!(report["speedtest_enabled"], false);
    assert_eq!(fixture.accepted.load(Ordering::SeqCst), 4);
    for flag in [
        "--allow-speedtest-staged",
        "--no-rootfs",
        "--speedtest",
        "--unknown",
    ] {
        let output = finish(fixture.command().arg(flag).spawn().unwrap());
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
    assert_eq!(fixture.accepted.load(Ordering::SeqCst), 4);
}

#[test]
fn external_process_stop_preserves_readable_partial_reports_and_stops_connections() {
    let fixture = Fixture::new();
    let mut child = fixture
        .command()
        .args(["--count", "8", "--concurrency", "2"])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let saved = fixture.read();
        if fixture.accepted.load(Ordering::SeqCst) >= 2
            && saved.as_ref().is_some_and(|report| {
                report["targets"][0]["samples"]
                    .as_array()
                    .is_some_and(|samples| !samples.is_empty())
            })
        {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("partial report was not saved");
        }
        thread::sleep(Duration::from_millis(5));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let saved = fixture.read().unwrap();
    assert_eq!(saved["complete"], false);
    assert!(saved["targets"][0]["samples"].as_array().unwrap().len() < 8);
    let before = fixture.accepted.load(Ordering::SeqCst);
    thread::sleep(Duration::from_millis(500));
    assert_eq!(fixture.accepted.load(Ordering::SeqCst), before);
    let section: Value = serde_json::from_slice(
        &fs::read(fixture.directory.join("sections/tcp_summary.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(section["complete"], false);
    assert!(serde_json::from_str::<Value>(section["text"].as_str().unwrap()).is_ok());
}

#[test]
fn build_information_is_available_without_a_workspace_or_network_probe() {
    let output = finish(
        Command::new(env!("CARGO_BIN_EXE_apsp-tcp-probe"))
            .arg("--build-info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    assert!(output.status.success());
    let info: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        info,
        json!({"version":"0.3.0","source_repo":"theLucius7/apsp","source_commit":option_env!("APSP_NATIVE_TCP_SOURCE_COMMIT")})
    );
}

#[cfg(unix)]
#[test]
fn blocked_error_output_does_not_prevent_bounded_process_exit() {
    use std::os::{fd::OwnedFd, unix::net::UnixStream};
    let fixture = Fixture::new();
    let (_reader, mut writer) = UnixStream::pair().unwrap();
    writer.set_nonblocking(true).unwrap();
    let buffer = [0; 4096];
    for bytes in [buffer.as_slice(), &buffer[..1]] {
        loop {
            match writer.write(bytes) {
                Ok(_) => (),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("fill stderr fixture: {error}"),
            }
        }
    }
    writer.set_nonblocking(false).unwrap();
    let started = Instant::now();
    let mut command = fixture.command();
    // Valid arguments with a missing input reach the bounded run error path.
    fs::remove_file(fixture.directory.join("targets.json")).unwrap();
    let output = finish(
        command
            .stderr(Stdio::from(OwnedFd::from(writer)))
            .spawn()
            .unwrap(),
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(fixture.accepted.load(Ordering::SeqCst), 0);
    assert!(fixture.read().is_none());
}

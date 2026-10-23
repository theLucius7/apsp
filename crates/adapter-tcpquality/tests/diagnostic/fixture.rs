use super::*;
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
};

pub(super) const SOURCE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub(super) struct Fixture {
    pub root: PathBuf,
    pub scope: Value,
    pub id: String,
}
impl Fixture {
    pub fn new(count: usize) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let number = NEXT.fetch_add(1, Ordering::SeqCst);
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("apsp-tcp-adapter-{}-{number}", std::process::id()));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let id = format!("00000000-0000-4000-8000-{number:012x}");
        let scope = json!({"schema":1,"targets":(0..count).map(|index| json!({
            "id":format!("10000000-0000-4000-8000-{index:012x}"),"name":format!("fixture {index}"),
            "target":"127.0.0.1","port":443,"carrier":"configured","region":null
        })).collect::<Vec<_>>()});
        let version = format!("0.3.0-{SOURCE}-r1");
        fs::create_dir(root.join(&version)).unwrap();
        fs::write(
            root.join(&version).join("apsp-tcp-probe"),
            b"fixture; never executed",
        )
        .unwrap();
        fs::set_permissions(
            root.join(&version).join("apsp-tcp-probe"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        Self { root, scope, id }
    }
    pub fn spec(&self) -> DiagnosticSpec {
        let bytes = self.scope.to_string();
        let version = format!("0.3.0-{SOURCE}-r1");
        DiagnosticSpec {
            id: self.id.clone(),
            version: version.clone(),
            binary_path: self.root.join(version).join("apsp-tcp-probe"),
            job_dir: self.root.join(&self.id),
            timeout_secs: 60,
            options: BTreeMap::from([
                ("ip_version".into(), "4".into()),
                ("targets".into(), bytes.clone()),
                (
                    "target_digest".into(),
                    format!("{:x}", Sha256::digest(bytes.as_bytes())),
                ),
            ]),
        }
    }
    pub fn workspace(&self) {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(self.spec().job_dir)
            .unwrap();
    }
    pub fn write(&self, name: &str, bytes: &[u8]) {
        let path = self.spec().job_dir.join(name);
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            fs::DirBuilder::new().mode(0o700).create(parent).unwrap();
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(bytes).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Default)]
pub(super) struct Recorder {
    pub probes: Mutex<Vec<String>>,
    pub created: Mutex<Vec<u32>>,
    pub written: Mutex<Vec<u32>>,
    pub bad: Option<Value>,
    pub bad_version: bool,
    pub truncated: bool,
    pub hang: bool,
}
impl Privileged for Recorder {
    fn execute<'a>(&'a self, _: &'a Path, _: &'a [String]) -> BoxFuture<'a, CommandOutput> {
        Box::pin(async { anyhow::bail!("unbounded execute is forbidden in this fixture") })
    }
    fn execute_bounded<'a>(
        &'a self,
        _: &'a Path,
        args: &'a [String],
        timeout_secs: u32,
        maximum: usize,
    ) -> BoxFuture<'a, Execution> {
        Box::pin(async move {
            assert_eq!(timeout_secs, 2);
            assert_eq!(maximum, 4096);
            assert_eq!(args.len(), 1);
            self.probes.lock().unwrap().push(args[0].clone());
            if self.hang {
                std::future::pending::<()>().await;
            }
            let stdout=match args[0].as_str() {
                "--version"=> if self.bad_version { "wrong version\n".into() } else { "apsp-tcp-probe 0.3.0\n".into() },
                "--build-info"=> self.bad.clone().unwrap_or_else(||json!({"version":"0.3.0","source_repo":"theLucius7/apsp","source_commit":SOURCE})).to_string(),
                _=>anyhow::bail!("unexpected execution"),
            };
            Ok(Execution {
                output: CommandOutput {
                    success: true,
                    stdout,
                    stderr: String::new(),
                },
                timed_out: false,
                truncated: self.truncated,
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
            self.created.lock().unwrap().push(mode);
            if !path.exists() {
                fs::DirBuilder::new().mode(mode).create(path)?;
            }
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
            self.written.lock().unwrap().push(mode);
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(mode)
                .open(path)?;
            file.write_all(bytes)?;
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
pub(super) fn report(spec: &DiagnosticSpec, scope: &Value) -> Value {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    json!({"schema":1,"method":"tcp_connect","semantics":"连接成功率和 TCP 建连耗时；不是包丢失率、吞吐测速或上游 TcpQuality 兼容评分。地区与运营商是管理员配置标签，未指定地区不推断。",
        "engine":{"name":"apsp-native-tcp-connect-v1","version":"0.3.0","source_commit":SOURCE},
        "started_at_ms":now,"finished_at_ms":null,
        "parameters":{"ip_version":"4","count":4,"concurrency":1,"dns_timeout_ms":2000,"connect_timeout_ms":1000,"interval_ms":250,"total_timeout_ms":60000},
        "target_digest":spec.options["target_digest"],"targets":scope["targets"].as_array().unwrap().iter().map(|target|json!({
            "target":target,"address":null,"dns_attempts":0,"status":"not_attempted","samples":[],
            "summary":{"attempted":0,"succeeded":0,"connection_success_percent":null,"latency_min_ms":null,"latency_mean_ms":null,"latency_max_ms":null},
            "error":null,"complete":false
        })).collect::<Vec<_>>(),"complete":false,"deadline_exceeded":false,"upload_enabled":false,"ranking_enabled":false,"speedtest_enabled":false})
}
pub(super) fn chapter(name: &str, text: &Value, complete: bool) -> Vec<u8> {
    serde_json::to_vec(
        &json!({"name":name,"text":text.to_string(),"complete":complete,"revision":1,
        "collected_at":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()}),
    )
    .unwrap()
}

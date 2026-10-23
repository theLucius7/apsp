use super::*;
use std::{fs, path::PathBuf};

pub(crate) struct Directory {
    pub path: PathBuf,
}
impl Directory {
    pub fn new() -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let root = fs::canonicalize(std::env::temp_dir()).unwrap();
        let path = root.join(format!(
            "apsp-native-tcp-test-{}-{}-{}",
            std::process::id(),
            crate::model::now_millis().unwrap(),
            SEQUENCE.fetch_add(1, Ordering::SeqCst)
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path).unwrap();
        Self { path }
    }
    pub fn input(&self, bytes: &[u8]) {
        use std::io::Write;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(self.path.join("targets.json"))
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }
    pub fn options(&self, bytes: &[u8], ip_version: IpVersion) -> Options {
        Options {
            workspace: self.path.clone(),
            targets_file: "targets.json".into(),
            target_digest: format!("{:x}", Sha256::digest(bytes)),
            ip_version,
            count: 4,
            concurrency: 1,
        }
    }
    pub async fn prepare(&self, targets: Vec<Target>, ip_version: IpVersion) -> (Options, Journal) {
        let bytes = serde_json::to_vec(&Snapshot { schema: 1, targets }).unwrap();
        self.input(&bytes);
        let options = self.options(&bytes, ip_version);
        let journal = Journal::open(&options).await.unwrap();
        (options, journal)
    }
    pub fn assert_reports(&self, report: &Report) {
        let body: serde_json::Value =
            serde_json::from_slice(&fs::read(self.path.join("result.json")).unwrap()).unwrap();
        let encoded = serde_json::to_vec_pretty(report).unwrap();
        assert_eq!(
            body,
            serde_json::from_slice::<serde_json::Value>(&encoded).unwrap()
        );
        assert_eq!(body["upload_enabled"], false);
        assert_eq!(body["ranking_enabled"], false);
        assert_eq!(body["speedtest_enabled"], false);
        assert!(
            body["started_at_ms"].as_u64().unwrap() > 0
                && body["finished_at_ms"].as_u64().unwrap() > 0
        );
        assert_eq!(body["target_digest"], report.target_digest);
        assert_eq!(body["engine"]["name"], "apsp-native-tcp-connect-v1");
        for entry in fs::read_dir(self.path.join("sections")).unwrap() {
            let path = entry.unwrap().path();
            assert!(
                !path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .contains("pending")
            );
            let bytes = fs::read(&path).unwrap();
            assert!(bytes.len() <= OUTPUT_LIMIT);
            let section: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert!(section["revision"].as_u64().unwrap() > 0);
            assert!(section["collected_at"].as_u64().unwrap() > 0);
            let name = section["name"].as_str().unwrap();
            assert!(
                name.len() <= 64
                    && name.bytes().all(|byte| byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'_')
            );
            assert!(
                serde_json::from_str::<serde_json::Value>(section["text"].as_str().unwrap())
                    .is_ok()
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
            }
        }
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
pub(crate) fn target(id: usize, host: &str, port: u16) -> Target {
    Target {
        id: format!("00000000-0000-4000-8000-{id:012x}"),
        name: format!("fixture {id}"),
        target: host.into(),
        port,
        carrier: "configured".into(),
        region: None,
    }
}

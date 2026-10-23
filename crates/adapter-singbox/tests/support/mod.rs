use anyhow::{Context, bail};
use apsp_adapter_sdk::{BoxFuture, CommandOutput, Privileged, RuntimeSpec, ServiceManager};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("apsp-adapter-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub fn spec(&self, stats_port: u16, inbounds: serde_json::Value) -> RuntimeSpec {
        let config = serde_json::json!({
            "log": {"level":"error"},
            "inbounds":inbounds,
            "outbounds":[{"type":"direct", "tag":"direct"}],
            "route":{"final":"direct"},
            "experimental":{"v2ray_api":{
                "listen":format!("127.0.0.1:{stats_port}"),
                "stats":{"enabled":true,"users":["u1_n3"]}
            }}
        })
        .to_string();
        std::fs::write(self.0.join("config.json"), &config).unwrap();
        RuntimeSpec {
            revision: 1,
            kernel_version: "1.14.2".into(),
            config_hash: "a".repeat(64),
            binary_path: self.0.join("sing-box"),
            revision_dir: self.0.clone(),
            stats_listen: format!("127.0.0.1:{stats_port}"),
            files: [("config.json".into(), config)].into(),
        }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct TestOps {
    pub version: Option<String>,
    pub check_ok: bool,
    pub commands: Mutex<Vec<Vec<String>>>,
}

impl Default for TestOps {
    fn default() -> Self {
        Self {
            version: Some("sing-box version 1.14.2\nTags: with_quic,with_v2ray_api\n".into()),
            check_ok: true,
            commands: Mutex::new(Vec::new()),
        }
    }
}

impl Privileged for TestOps {
    fn execute<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
    ) -> BoxFuture<'a, CommandOutput> {
        Box::pin(async move {
            self.commands.lock().unwrap().push(args.to_vec());
            if let Some(version) = &self.version {
                return Ok(CommandOutput {
                    success: args[0] == "version" || self.check_ok,
                    stdout: if args[0] == "version" {
                        version.clone()
                    } else {
                        String::new()
                    },
                    stderr: String::new(),
                });
            }
            let output = tokio::time::timeout(
                Duration::from_secs(10),
                tokio::process::Command::new(program)
                    .args(args)
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .context("test command timed out")??;
            Ok(CommandOutput {
                success: output.status.success(),
                stdout: String::from_utf8(output.stdout)?,
                stderr: String::from_utf8(output.stderr)?,
            })
        })
    }

    fn create_dir<'a>(
        &'a self,
        _path: &'a Path,
        _mode: u32,
        _group: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async { bail!("adapter must not write files") })
    }
    fn write_file<'a>(
        &'a self,
        _path: &'a Path,
        _bytes: &'a [u8],
        _mode: u32,
        _group: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async { bail!("adapter must not write files") })
    }
    fn atomic_symlink<'a>(&'a self, _link: &'a Path, _target: &'a Path) -> BoxFuture<'a, ()> {
        Box::pin(async { bail!("adapter must not write files") })
    }
    fn remove_symlink<'a>(&'a self, _link: &'a Path) -> BoxFuture<'a, ()> {
        Box::pin(async { bail!("adapter must not write files") })
    }
    fn install_archive<'a>(
        &'a self,
        _archive: &'a Path,
        _directory: &'a Path,
        _binary_name: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async { bail!("adapter must not write files") })
    }
}

pub struct TestServices {
    pub active: bool,
    pub pid: Option<u32>,
    pub calls: Mutex<Vec<String>>,
}

impl Default for TestServices {
    fn default() -> Self {
        Self {
            active: true,
            pid: None,
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl ServiceManager for TestServices {
    fn reload<'a>(&'a self, unit: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            assert_eq!(unit, "apsp-singbox@main.service");
            self.calls.lock().unwrap().push("reload".into());
            if let Some(pid) = self.pid {
                let status = tokio::time::timeout(
                    Duration::from_secs(3),
                    tokio::process::Command::new("kill")
                        .args(["-HUP", &pid.to_string()])
                        .kill_on_drop(true)
                        .status(),
                )
                .await??;
                anyhow::ensure!(status.success(), "test HUP failed");
            }
            Ok(())
        })
    }
    fn restart<'a>(&'a self, unit: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            assert_eq!(unit, "apsp-singbox@main.service");
            self.calls.lock().unwrap().push("restart".into());
            Ok(())
        })
    }
    fn stop<'a>(&'a self, _unit: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.calls.lock().unwrap().push("stop".into());
            Ok(())
        })
    }
    fn is_active<'a>(&'a self, unit: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            assert_eq!(unit, "apsp-singbox@main.service");
            Ok(self.active)
        })
    }
}

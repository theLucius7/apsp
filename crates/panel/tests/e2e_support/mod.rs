use crate::{release_fixture, release_support};

use anyhow::{Context, Result};
use apsp_adapter_sdk::{
    Adapter, BoxFuture, Descriptor, Plan, Prepared, Privileged, RuntimeSpec, ServiceManager,
    UsageSource,
};
use apsp_agent_core::{
    config::Config as AgentConfig,
    fake::{FakeAdapter, FakeServiceManager},
    system::SystemOps,
    transport,
};
use apsp_panel::{AppState, config::Config, publisher, router};
use flate2::{Compression, write::GzEncoder};
use reqwest::{Client, Method, header};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{fs, future::Future, path::PathBuf, sync::Arc, time::Duration};
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;

pub struct Harness {
    pub state: AppState,
    pub base: String,
    pub client: Client,
    pub cookie: String,
    pub directory: PathBuf,
    tasks: Vec<JoinHandle<()>>,
}

impl Harness {
    pub async fn start(pool: PgPool) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let listen = listener.local_addr()?;
        let base = format!("http://{listen}");
        // A short root also stays below macOS's Unix socket path length limit.
        let directory = PathBuf::from("/tmp").join(format!("sn-e2e-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory)?;
        let directory = directory.canonicalize()?;
        let mut state = AppState::new(
            pool,
            Config {
                database_url: String::new(),
                listen,
                public_url: base.clone(),
                data_dir: directory.join("panel"),
                admin_password: Some("end-to-end-test-password".into()),
            },
        )
        .await?;
        state.release_keys = Some(Arc::new(release_support::trusted_keys()));
        let app = router(state.clone());
        let http = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .expect("end-to-end HTTP server");
        });
        let publisher_state = state.clone();
        let publisher = tokio::spawn(async move {
            publisher::run(publisher_state).await;
        });
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()?;
        let response = client
            .post(format!("{base}/api/login"))
            .json(&json!({"password": "end-to-end-test-password"}))
            .send()
            .await?
            .error_for_status()?;
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .context("admin session cookie")?
            .to_str()?
            .split(';')
            .next()
            .context("session cookie value")?
            .to_owned();
        Ok(Self {
            state,
            base,
            client,
            cookie,
            directory,
            tasks: vec![http, publisher],
        })
    }

    pub async fn api(&self, method: Method, path: &str, body: Value) -> Result<Value> {
        let response = self
            .client
            .request(method, format!("{}{path}", self.base))
            .header(header::COOKIE, &self.cookie)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        anyhow::ensure!(status.is_success(), "{path} returned {status}: {text}");
        Ok(serde_json::from_str(&text)?)
    }

    pub fn agent_config(&self) -> AgentConfig {
        let root = self.directory.join("agent");
        AgentConfig {
            settings: apsp_protocol::AgentSettings {
                discover_public_ips: false,
                ..Default::default()
            },
            panel_url: self.base.clone(),
            panel_ca_file: None,
            identity_dir: root.join("identity"),
            state_db: root.join("state.db"),
            runtime_root: root.join("runtime"),
            install_root: root.join("install"),
            agent_root: root.join("core"),
            status_socket: root.join("run/status.sock"),
            operation_timeout_secs: 5,
            public_ips: Vec::new(),
            allow_remote_commands: false,
        }
    }

    pub fn write_runtime_artifact(&self) -> Result<Vec<u8>> {
        let binary = b"#!/bin/sh\nexit 0\n";
        let mut archive = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        let mut header = tar::Header::new_gnu();
        header.set_size(binary.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive.append_data(&mut header, "demo", binary.as_slice())?;
        let archive = archive.into_inner()?.finish()?;
        let mut entry =
            release_support::entry("sing-box", "1.14.2", "demo", "tar.gz", &archive, binary);
        // Real telemetry reports the host ABI; this fake runtime must use that signed identity.
        entry.arch = apsp_protocol::release::native_target()?;
        entry.asset_name = apsp_protocol::release::canonical_asset_name(&entry)?;
        release_fixture::write_entries(&self.state.config.data_dir, vec![(entry, archive)])?;
        Ok(binary.to_vec())
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[derive(Default)]
pub struct PanelAdapter(pub FakeAdapter);

impl Adapter for PanelAdapter {
    fn describe(&self) -> Descriptor {
        Descriptor {
            module: "singbox".into(),
            plugin_name: "sing-box".into(),
            ..self.0.describe()
        }
    }
    fn prepare<'a>(
        &'a self,
        runtime: RuntimeSpec,
        privileged: &'a dyn Privileged,
    ) -> BoxFuture<'a, Prepared> {
        self.0.prepare(runtime, privileged)
    }
    fn plan<'a>(
        &'a self,
        previous: Option<&'a Prepared>,
        target: &'a Prepared,
    ) -> BoxFuture<'a, Plan> {
        self.0.plan(previous, target)
    }
    fn apply<'a>(
        &'a self,
        plan: Plan,
        target: &'a Prepared,
        services: &'a dyn ServiceManager,
    ) -> BoxFuture<'a, ()> {
        self.0.apply(plan, target, services)
    }
    fn health<'a>(
        &'a self,
        target: &'a Prepared,
        services: &'a dyn ServiceManager,
    ) -> BoxFuture<'a, bool> {
        self.0.health(target, services)
    }
    fn usage_source(&self) -> Option<&dyn UsageSource> {
        Some(&self.0)
    }
}

pub struct AgentTask(JoinHandle<Result<()>>);

impl AgentTask {
    pub fn start(
        config: AgentConfig,
        adapter: Arc<PanelAdapter>,
        services: Arc<FakeServiceManager>,
    ) -> Self {
        Self(tokio::spawn(transport::run(
            config,
            vec![adapter],
            Arc::new(SystemOps),
            services,
            "panel-test-agent",
        )))
    }

    pub async fn stop(self) -> Result<()> {
        self.0.abort();
        // Awaiting through a mutable borrow preserves this guard's abort-on-drop behavior.
        let mut task = self;
        match (&mut task.0).await {
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(error.into()),
            Ok(result) => result.context("agent stopped before cancellation"),
        }
    }
}

impl Drop for AgentTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub async fn eventually<F, Fut>(label: &str, seconds: u64, mut check: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool>>,
{
    tokio::time::timeout(Duration::from_secs(seconds), async {
        loop {
            if check().await? {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .with_context(|| format!("timed out waiting for {label}"))?
}

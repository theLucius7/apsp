#![forbid(unsafe_code)]

#[allow(dead_code)]
mod e2e_support;
mod release_fixture;
#[path = "../../protocol/tests/support/release.rs"]
mod release_support;

use anyhow::{Context, Result};
use apsp_adapter_nodequality::{NodeQualityAdapter, VERSION};
use apsp_adapter_sdk::{BoxFuture, JobStatus, ServiceJob, ServiceManager};
use apsp_agent_core::{
    Config, State, fake::FakeResourceOps, identity, system::SystemOps, transport,
};
use e2e_support::{Harness, eventually};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default, Serialize, Deserialize)]
struct ServicesState {
    starts: usize,
    stops: usize,
    jobs: BTreeMap<String, (PathBuf, JobStatus)>,
}

#[derive(Clone)]
struct IndependentServices {
    root: PathBuf,
}

impl IndependentServices {
    fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        let services = Self { root };
        if !services.state_path().exists() {
            services.save_state(&ServicesState::default())?;
        }
        Ok(services)
    }

    fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    fn block_stop_path(&self) -> PathBuf {
        self.root.join("block_stop")
    }

    fn load_state(&self) -> Result<ServicesState> {
        let bytes = fs::read(self.state_path())?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn save_state(&self, state: &ServicesState) -> Result<()> {
        let tmp = self.root.join(format!(
            "state.tmp.{}.{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::write(&tmp, serde_json::to_vec(state)?)?;
        fs::rename(&tmp, self.state_path())?;
        Ok(())
    }

    fn starts(&self) -> Result<usize> {
        Ok(self.load_state()?.starts)
    }

    fn stops(&self) -> Result<usize> {
        Ok(self.load_state()?.stops)
    }

    fn jobs(&self) -> Result<BTreeMap<String, (PathBuf, JobStatus)>> {
        Ok(self.load_state()?.jobs)
    }

    fn set_block_stop(&self, block: bool) -> Result<()> {
        let path = self.block_stop_path();
        if block {
            fs::write(path, b"1")?;
        } else if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    fn block_stop(&self) -> bool {
        self.block_stop_path().exists()
    }

    fn finish(&self) -> Result<()> {
        let mut state = self.load_state()?;
        for (directory, status) in state.jobs.values_mut() {
            fs::write(
                directory.join("result.txt"),
                "# NodeQuality\n\nHardware, IP and network fixture report.\n",
            )?;
            fs::write(
                directory.join("report-url.txt"),
                "https://nodequality.com/r/EXAMPLE_REPORT\n",
            )?;
            *status = JobStatus::Succeeded;
        }
        self.save_state(&state)
    }
}

impl ServiceManager for IndependentServices {
    fn running_diagnostic_units(&self) -> BoxFuture<'_, Vec<String>> {
        Box::pin(async {
            Ok(self
                .load_state()?
                .jobs
                .into_iter()
                .filter(|(_, (_, status))| *status == JobStatus::Running)
                .map(|(unit, _)| unit)
                .collect())
        })
    }
    fn reload<'a>(&'a self, _: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
    fn restart<'a>(&'a self, _: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
    fn supports_confirmed_cancellation(&self) -> bool {
        true
    }
    fn supports_diagnostic_cpu_ceiling(&self) -> bool {
        // TEST_ONLY explicit simulated backend; no native controller is probed.
        true
    }
    fn diagnostic_cleanup_confirmed<'a>(
        &'a self,
        unit: &'a str,
        _: &'a std::path::Path,
    ) -> BoxFuture<'a, bool> {
        Box::pin(async move { Ok(!self.load_state()?.jobs.contains_key(unit)) })
    }
    fn stop<'a>(&'a self, unit: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut state = self.load_state()?;
            state.stops += 1;
            self.save_state(&state)?;
            anyhow::ensure!(!self.block_stop(), "fixture stop not confirmed");
            state.jobs.remove(unit);
            self.save_state(&state)
        })
    }
    fn is_active<'a>(&'a self, _: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async { Ok(false) })
    }
    fn start_job<'a>(&'a self, job: &'a ServiceJob) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            // TEST_ONLY: this backend records a bounded daily service contract;
            // it does not start a native service or certify a host CPU controller.
            assert_eq!(job.timeout_secs, 90);
            assert_eq!(job.memory_max.get(), 64 * 1024 * 1024);
            assert_eq!(job.tasks_max.get(), 32);
            assert_eq!(job.cpu_max_percent.get(), 100);
            assert_eq!(job.cpu_weight.get(), 10);
            assert_eq!(job.io_weight.get(), 10);
            assert_eq!(job.oom_score_adjust.get(), 500);
            assert!(job.args.windows(2).any(|args| args == ["--mode", "daily"]));
            let mut state = self.load_state()?;
            anyhow::ensure!(
                !state.jobs.contains_key(&job.unit),
                "duplicate service start"
            );
            state.jobs.insert(
                job.unit.clone(),
                (job.working_directory.clone(), JobStatus::Running),
            );
            state.starts += 1;
            self.save_state(&state)
        })
    }
    fn job_status<'a>(&'a self, unit: &'a str) -> BoxFuture<'a, JobStatus> {
        Box::pin(async move {
            Ok(self
                .load_state()?
                .jobs
                .get(unit)
                .map(|(_, status)| status.clone())
                .unwrap_or(JobStatus::Missing))
        })
    }
}

const NODEQUALITY_SCRIPT: &[u8] =
    b"#!/bin/bash\nprintf 'nodequality a92fca6c0067df29ddd03fdc2fee6f3000f64545-r22\\n'\n";
const SUBPROCESS_DIR_ENV: &str = "APSP_E2E_SUBPROCESS_DIR";
const SUBPROCESS_BUNDLE_FOOTER_MAGIC: &[u8; 16] = b"APSPE2EBUNDLE001";

fn signed_agent_target() -> (&'static str, &'static str) {
    let env = if cfg!(target_env = "gnu") {
        "gnu"
    } else {
        "musl"
    };
    let target = match (cfg!(target_arch = "aarch64"), cfg!(target_env = "gnu")) {
        (true, true) => "linux-gnu-arm64",
        (true, false) => "linux-musl-arm64",
        (false, true) => "linux-gnu-amd64",
        (false, false) => "linux-musl-amd64",
    };
    (target, env)
}

struct SignedAgentFixture {
    exe_path: PathBuf,
    control_dir: PathBuf,
    services: IndependentServices,
}

struct SpawnedAgent {
    child: Mutex<tokio::process::Child>,
}

impl SpawnedAgent {
    fn is_finished(&self) -> Result<bool> {
        Ok(self.child.lock().unwrap().try_wait()?.is_some())
    }

    async fn stop(self) {
        let mut child = self.child.into_inner().unwrap();
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}

impl SignedAgentFixture {
    fn prepare(harness: &Harness) -> Result<Self> {
        use apsp_protocol::{
            bundled::{extract_embedded_agent_manifest, extract_embedded_agent_manifest_optional},
            release::canonical_asset_name,
        };
        use release_fixture::{
            archive, build_signed_agent_entry, encode_embedded_agent_manifest_frame,
            pinned_components, write_entries_tagged,
        };

        let base_exe = fs::read(std::env::current_exe()?)?;
        anyhow::ensure!(
            extract_embedded_agent_manifest_optional(&base_exe)
                .map_err(anyhow::Error::msg)?
                .is_none(),
            "base test executable must not contain an embedded manifest frame"
        );

        let (target, _) = signed_agent_target();
        let tar_gz = archive("nodequality", NODEQUALITY_SCRIPT)?;
        let tar_sha = release_support::hash(&tar_gz);
        let mut components = pinned_components(&["nodequality"])?;
        assert_eq!(components["nodequality"].version, VERSION);
        components
            .get_mut("nodequality")
            .context("pinned nodequality component")?
            .sha256 = Some(tar_sha);
        let (_, template) = build_signed_agent_entry("0.4.0", target, components)?;
        let raw = &template.raw_binary;
        let manifest = extract_embedded_agent_manifest(raw).map_err(anyhow::Error::msg)?;
        let frame = encode_embedded_agent_manifest_frame(&manifest).map_err(anyhow::Error::msg)?;

        let mut agent_bytes = base_exe;
        agent_bytes.extend_from_slice(&tar_gz);
        agent_bytes.extend_from_slice(&frame);
        agent_bytes.extend_from_slice(&(tar_gz.len() as u64).to_le_bytes());
        agent_bytes.extend_from_slice(&(frame.len() as u64).to_le_bytes());
        agent_bytes.extend_from_slice(SUBPROCESS_BUNDLE_FOOTER_MAGIC);
        assert_eq!(
            extract_embedded_agent_manifest(&agent_bytes).map_err(anyhow::Error::msg)?,
            manifest
        );

        let control_dir = harness.directory.join("agent-subprocess");
        fs::create_dir_all(&control_dir)?;
        let exe_path = control_dir.join("apsp-agent");
        {
            let mut file = fs::File::create(&exe_path)?;
            file.write_all(&agent_bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(0o755))?;
            }
            file.sync_all()?;
        }

        let mut entry = release_support::entry(
            "agent",
            "0.4.0",
            "apsp-agent",
            "raw",
            &agent_bytes,
            &agent_bytes,
        );
        entry.arch = target.into();
        entry.asset_name = canonical_asset_name(&entry)?;
        write_entries_tagged(
            &harness.state.config.data_dir,
            "agent-v0.4.0",
            vec![(entry, agent_bytes)],
        )?;

        let services = IndependentServices::open(control_dir.join("services"))?;
        Ok(Self {
            exe_path,
            control_dir,
            services,
        })
    }

    fn spawn(&self, config: &Config) -> Result<SpawnedAgent> {
        fs::write(
            self.control_dir.join("agent-config.json"),
            serde_json::to_vec(config)?,
        )?;
        let child = tokio::process::Command::new(&self.exe_path)
            .arg("--exact")
            .arg("agent_subprocess_entrypoint")
            .arg("--nocapture")
            .env(SUBPROCESS_DIR_ENV, &self.control_dir)
            .kill_on_drop(true)
            .spawn()
            .context("spawn signed Agent subprocess")?;
        Ok(SpawnedAgent {
            child: Mutex::new(child),
        })
    }
}

fn extract_subprocess_nodequality(
    self_bytes: &[u8],
) -> Result<apsp_agent_core::bundled::BundledComponents> {
    use apsp_agent_core::bundled::{BundledComponents, BundledItem, BundledPayload};

    let manifest = apsp_protocol::bundled::extract_embedded_agent_manifest(self_bytes)
        .map_err(anyhow::Error::msg)?;
    let footer_len = 16 + SUBPROCESS_BUNDLE_FOOTER_MAGIC.len();
    anyhow::ensure!(
        self_bytes.len() > footer_len
            && &self_bytes[self_bytes.len() - SUBPROCESS_BUNDLE_FOOTER_MAGIC.len()..]
                == SUBPROCESS_BUNDLE_FOOTER_MAGIC,
        "signed subprocess executable is missing bundle footer"
    );
    let meta_end = self_bytes.len() - SUBPROCESS_BUNDLE_FOOTER_MAGIC.len();
    let tar_len = u64::from_le_bytes(
        self_bytes[meta_end - 16..meta_end - 8]
            .try_into()
            .expect("tar_len bytes"),
    ) as usize;
    let frame_len = u64::from_le_bytes(
        self_bytes[meta_end - 8..meta_end]
            .try_into()
            .expect("frame_len bytes"),
    ) as usize;
    let payload_end = meta_end
        .checked_sub(16)
        .and_then(|end| end.checked_sub(frame_len))
        .context("invalid frame length in subprocess footer")?;
    let payload_start = payload_end
        .checked_sub(tar_len)
        .context("invalid archive length in subprocess footer")?;
    let tar_gz = self_bytes[payload_start..payload_end].to_vec();

    let record = manifest
        .components
        .get("nodequality")
        .context("missing nodequality record in embedded manifest")?;
    let expected_sha = record
        .sha256
        .as_ref()
        .context("missing nodequality sha256 in embedded manifest")?
        .clone();
    anyhow::ensure!(
        release_support::hash(&tar_gz) == expected_sha,
        "embedded nodequality archive digest mismatch"
    );

    Ok(BundledComponents {
        runtimes: BTreeMap::new(),
        tools: BTreeMap::from([(
            "nodequality".into(),
            BundledItem {
                artifact_name: "nodequality".into(),
                binary_name: "nodequality".into(),
                version: record.version.clone(),
                payload: BundledPayload::TarGz {
                    archive_sha256: expected_sha,
                    bytes: std::borrow::Cow::Owned(tar_gz),
                },
            },
        )]),
    })
}

#[test]
fn agent_subprocess_entrypoint() -> Result<()> {
    let Some(control_dir) = std::env::var_os(SUBPROCESS_DIR_ENV) else {
        return Ok(());
    };
    let control_dir = PathBuf::from(control_dir);
    let self_bytes = fs::read(std::env::current_exe()?)?;
    let bundled = extract_subprocess_nodequality(&self_bytes)?;
    let config: Config = serde_json::from_slice(&fs::read(control_dir.join("agent-config.json"))?)?;
    let services = Arc::new(IndependentServices::open(control_dir.join("services"))?);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(transport::run_with_components(
        config,
        vec![],
        vec![Arc::new(NodeQualityAdapter::new())],
        bundled,
        Arc::new(FakeResourceOps::new(Arc::new(SystemOps))),
        services,
        "0.4.0",
    ))
}

// IndependentServices simulates the Linux job backend even on a macOS test host.
async fn mark_simulated_linux(harness: &Harness, id: i64) -> Result<()> {
    let (_, env) = signed_agent_target();
    sqlx::query("UPDATE servers SET static_info=static_info || $2 WHERE id=$1")
        .bind(id)
        .bind(json!({"os": "linux", "libc": env}))
        .execute(&harness.state.pool)
        .await?;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn node_report_survives_agent_restart_and_is_started_only_once(pool: PgPool) -> Result<()> {
    let harness = Harness::start(pool).await?;
    let fixture = SignedAgentFixture::prepare(&harness)?;
    let server = harness
        .api(Method::POST, "/api/servers", json!({"name":"诊断测试节点"}))
        .await?;
    let id = server["id"].as_i64().context("server id")?;
    let token = harness
        .api(
            Method::POST,
            &format!("/api/servers/{id}/enrollment"),
            json!({}),
        )
        .await?;
    let mut config = harness.agent_config();
    config.public_ips = vec!["192.0.2.10".into(), "2001:db8::10".into()];
    identity::enroll(
        &config,
        token["token"].as_str().context("enrollment token")?,
    )
    .await?;
    let agent = fixture.spawn(&config)?;
    eventually(
        "diagnostic capability and reported addresses",
        15,
        || async {
            anyhow::ensure!(
                !agent.is_finished()?,
                "signed Agent subprocess exited during startup"
            );
            let row: (serde_json::Value, serde_json::Value, Option<i64>) = sqlx::query_as(
                "SELECT capabilities,static_info,static_info_received_at FROM servers WHERE id=$1",
            )
            .bind(id)
            .fetch_one(&harness.state.pool)
            .await?;
            Ok(row.2.is_some()
                && row.0.as_array().is_some_and(|caps| {
                    caps.iter().any(|cap| cap == "diagnostic:nodequality")
                        && caps
                            .iter()
                            .any(|cap| cap == apsp_protocol::SELF_CONTAINED_AGENT_CAPABILITY)
                        && caps
                            .iter()
                            .any(|cap| cap == apsp_protocol::DIAGNOSTIC_CPU_CEILING_CAPABILITY)
                })
                && row.1["ip_addresses"]
                    .as_array()
                    .is_some_and(|ips| ips.iter().any(|ip| ip == "192.0.2.10")))
        },
    )
    .await?;
    mark_simulated_linux(&harness, id).await?;
    let report = harness
        .api(
            Method::POST,
            &format!("/api/servers/{id}/node-quality/reports"),
            json!({"mode":"daily","ip_version":"both"}),
        )
        .await?;
    let report_id = report["id"].as_str().context("report job id")?.to_owned();
    eventually("independent report service start", 15, || async {
        Ok(fixture.services.starts()? == 1)
    })
    .await?;
    agent.stop().await;
    assert!(
        State::open(&config.state_db)?
            .get_json::<serde_json::Value>("diagnostics:active")?
            .is_some_and(|checkpoint| !checkpoint.is_null()),
        "diagnostic-only restart must recover an existing managed checkpoint"
    );
    let mut state = State::open(&config.state_db)?;
    let mut saved: serde_json::Value = state
        .get_json("diagnostics:active")?
        .context("saved start")?;
    saved["Started"]["spec"]["version"] = json!("a92fca6c0067df29ddd03fdc2fee6f3000f64545-r2");
    saved["Started"]["spec"]["options"] = json!({});
    state.set_json("diagnostics:active", &saved)?;
    drop(state);
    sqlx::query("UPDATE diagnostic_jobs SET status='running',job=jsonb_set(jsonb_set(job-'agent_bundle','{options}','{}'),'{version}','\"a92fca6c0067df29ddd03fdc2fee6f3000f64545-r2\"') WHERE id=$1")
        .bind(uuid::Uuid::parse_str(&report_id)?).execute(&harness.state.pool).await?;
    fixture.services.finish()?;
    let restarted = fixture.spawn(&config)?;
    eventually(
        "completed report returned after agent restart",
        20,
        || async {
            anyhow::ensure!(
                !restarted.is_finished()?,
                "diagnostic-only Agent exited while recovering its report"
            );
            let row: (String, Option<serde_json::Value>) =
                sqlx::query_as("SELECT status,report FROM diagnostic_jobs WHERE id=$1")
                    .bind(uuid::Uuid::parse_str(&report_id)?)
                    .fetch_one(&harness.state.pool)
                    .await?;
            Ok(row.0 == "succeeded"
                && row.1.is_some_and(|report| {
                    report["text"]
                        .as_str()
                        .is_some_and(|text| text.contains("fixture report"))
                }))
        },
    )
    .await?;
    assert_eq!(fixture.services.starts()?, 1);
    let detail = harness
        .api(
            Method::GET,
            &format!("/api/servers/{id}/node-quality"),
            json!({}),
        )
        .await?;
    assert!(detail.to_string().contains("EXAMPLE_REPORT"));
    restarted.stop().await;
    Ok(())
}

#[path = "diagnostic_end_to_end/cancellation.rs"]
mod cancellation;

#[path = "diagnostic_end_to_end/chain_gate.rs"]
mod chain_gate;

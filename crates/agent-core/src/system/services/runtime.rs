use super::{ServiceBackend, SystemServiceManager};
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::RuntimeInstance;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, PartialEq, Eq)]
struct ServiceIdentity {
    pid: u32,
    control_group: String,
    invocation_id: String,
}

fn service_identity(text: &str) -> Result<ServiceIdentity> {
    ensure!(
        text.len() <= 16384,
        "runtime status exceeds its byte budget"
    );
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line
            .split_once('=')
            .context("invalid runtime status line")?;
        ensure!(
            values.insert(key, value).is_none(),
            "duplicate runtime status field"
        );
    }
    ensure!(
        values.get("LoadState") == Some(&"loaded")
            && values.get("ActiveState") == Some(&"active")
            && values.get("SubState") == Some(&"running"),
        "runtime is not fully running"
    );
    let pid: u32 = values
        .get("MainPID")
        .context("missing runtime PID")?
        .parse()?;
    let control_pid: u32 = values
        .get("ControlPID")
        .context("missing control PID")?
        .parse()?;
    ensure!(
        pid != 0 && control_pid == 0,
        "runtime has no main process or a pending control operation"
    );
    let group = values
        .get("ControlGroup")
        .context("missing runtime cgroup")?;
    ensure!(
        group.starts_with('/') && group.len() <= 4096 && !group.contains(['\0', '\r', '\n']),
        "invalid runtime cgroup"
    );
    let invocation = values
        .get("InvocationID")
        .context("missing runtime invocation")?;
    ensure!(
        invocation.len() == 32
            && invocation.bytes().all(|byte| byte.is_ascii_hexdigit())
            && invocation.bytes().any(|byte| byte != b'0'),
        "invalid runtime invocation"
    );
    Ok(ServiceIdentity {
        pid,
        control_group: group.to_string(),
        invocation_id: invocation.to_string(),
    })
}

async fn observe(manager: &SystemServiceManager, unit: &str) -> Result<ServiceIdentity> {
    let service = unit.strip_suffix(".service").unwrap_or(unit);
    ensure!(
        !service.is_empty()
            && !matches!(service, "." | "..")
            && !service.starts_with('-')
            && unit.len() <= 255
            && unit
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'.' | b'_' | b'-' | b'@')),
        "invalid service unit"
    );
    let args = [
        "show".to_owned(),
        "--property=LoadState,ActiveState,SubState,MainPID,ControlPID,ControlGroup,InvocationID"
            .to_owned(),
        "--".to_owned(),
        unit.to_owned(),
    ];
    let result = manager
        .privileged
        .execute_bounded(Path::new("systemctl"), &args, 10, 16384)
        .await?;
    ensure!(
        !result.timed_out && !result.truncated && result.output.success,
        "runtime service identity is unavailable"
    );
    service_identity(&result.output.stdout)
}

pub(super) async fn inspect(manager: &SystemServiceManager, unit: &str) -> Result<RuntimeInstance> {
    ensure!(
        cfg!(target_os = "linux") && manager.backend == ServiceBackend::Systemd,
        "exact runtime inspection requires Linux systemd"
    );
    let service = observe(manager, unit).await?;
    let first = manager
        .privileged
        .runtime_process(service.pid, &service.control_group)
        .await?;
    ensure!(
        service == observe(manager, unit).await?,
        "runtime service changed during inspection"
    );
    let second = manager
        .privileged
        .runtime_process(service.pid, &service.control_group)
        .await?;
    ensure!(first == second, "runtime process changed during inspection");
    let instance_id = format!(
        "{:x}",
        Sha256::digest(
            format!(
                "apsp-service-instance-v1\0{}\0{}",
                service.invocation_id, first.instance_id
            )
            .as_bytes()
        )
    );
    Ok(RuntimeInstance {
        instance_id,
        ..first
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> String {
        "LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=12\nControlPID=0\nControlGroup=/system.slice/example-runtime.service\nInvocationID=0123456789abcdef0123456789abcdef\n".into()
    }

    #[test]
    fn requires_one_fully_running_controlled_instance() {
        assert_eq!(service_identity(&status()).unwrap().pid, 12);
        for (old, new) in [
            ("loaded", "masked"),
            ("active", "activating"),
            ("running", "exited"),
            ("MainPID=12", "MainPID=0"),
            ("ControlPID=0", "ControlPID=9"),
            ("/system.slice/example-runtime.service", "relative"),
            (
                "0123456789abcdef0123456789abcdef",
                "00000000000000000000000000000000",
            ),
        ] {
            assert!(
                service_identity(&status().replace(old, new)).is_err(),
                "{old}"
            );
        }
        assert!(service_identity(&(status() + "MainPID=12\n")).is_err());
        assert!(service_identity(&"x".repeat(16385)).is_err());
    }

    #[tokio::test]
    async fn unsupported_backends_never_claim_exact_inspection() {
        let operations = std::sync::Arc::new(crate::system::SystemOps);
        for backend in [
            ServiceBackend::OpenRc,
            ServiceBackend::Launchd,
            ServiceBackend::FreeBsd,
            ServiceBackend::WindowsTask,
            ServiceBackend::Unmanaged,
        ] {
            let manager = SystemServiceManager::new(operations.clone(), backend);
            assert!(!apsp_adapter_sdk::ServiceManager::supports_runtime_checkpoint(&manager));
            assert!(inspect(&manager, "example-runtime.service").await.is_err());
        }
        if !cfg!(target_os = "linux") {
            let manager = SystemServiceManager::new(operations, ServiceBackend::Systemd);
            assert!(!apsp_adapter_sdk::ServiceManager::supports_runtime_checkpoint(&manager));
            assert!(inspect(&manager, "example-runtime.service").await.is_err());
        }
    }
}

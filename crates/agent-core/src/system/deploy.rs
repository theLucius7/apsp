#[path = "deploy/native.rs"]
mod native;
use crate::{
    Config,
    system::{ServiceBackend, SystemOps, SystemServiceManager},
};
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::{Descriptor, Privileged, ServiceManager};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub fn core_root(config: &Config) -> Result<PathBuf> {
    Ok(config.agent_root.clone())
}
pub fn executable_name() -> &'static str {
    if cfg!(windows) {
        "apsp-agent.exe"
    } else {
        "apsp-agent"
    }
}

async fn ensure_linux_account(ops: &dyn Privileged, name: &str) -> Result<()> {
    ensure!(
        crate::artifacts::safe_component(name),
        "invalid runtime service account"
    );
    let group_exists = ops
        .execute(Path::new("getent"), &["group".into(), name.into()])
        .await
        .is_ok_and(|out| out.success);
    if !group_exists {
        let created = ops
            .execute(Path::new("groupadd"), &["--system".into(), name.into()])
            .await;
        if !created.is_ok_and(|out| out.success) {
            let alpine = ops
                .execute(Path::new("addgroup"), &["-S".into(), name.into()])
                .await?;
            ensure!(
                alpine.success,
                "cannot create runtime system group: {}",
                alpine.stderr
            );
        }
    }
    let account_exists = ops
        .execute(Path::new("id"), &["-u".into(), name.into()])
        .await
        .is_ok_and(|out| out.success);
    if !account_exists {
        let nologin = if Path::new("/usr/sbin/nologin").exists() {
            "/usr/sbin/nologin"
        } else {
            "/sbin/nologin"
        };
        let created = ops
            .execute(
                Path::new("useradd"),
                &[
                    "--system".into(),
                    "--no-create-home".into(),
                    "--gid".into(),
                    name.into(),
                    "--shell".into(),
                    nologin.into(),
                    name.into(),
                ],
            )
            .await;
        if !created.is_ok_and(|out| out.success) {
            let alpine = ops
                .execute(
                    Path::new("adduser"),
                    &[
                        "-S".into(),
                        "-D".into(),
                        "-H".into(),
                        "-G".into(),
                        name.into(),
                        "-s".into(),
                        nologin.into(),
                        name.into(),
                    ],
                )
                .await?;
            ensure!(
                alpine.success,
                "cannot create runtime system account: {}",
                alpine.stderr
            );
        }
    }
    Ok(())
}

pub(crate) async fn ensure_runtime_registered(
    ops: &dyn Privileged,
    backend: ServiceBackend,
    spec: &apsp_adapter_sdk::RuntimeServiceSpec,
) -> Result<()> {
    ensure!(
        backend != ServiceBackend::Unmanaged,
        "monitor-only Agent has no runtime service manager"
    );
    ensure!(
        crate::artifacts::safe_component(&spec.descriptor.artifact_name)
            && crate::artifacts::safe_component(&spec.descriptor.binary_name)
            && (spec.descriptor.service_group.is_empty()
                || crate::artifacts::safe_component(&spec.descriptor.service_group)),
        "invalid runtime service descriptor"
    );
    if !spec.descriptor.service_group.is_empty() {
        match backend {
            ServiceBackend::Systemd | ServiceBackend::OpenRc => {
                ensure_linux_account(ops, &spec.descriptor.service_group).await?;
            }
            ServiceBackend::Launchd | ServiceBackend::FreeBsd | ServiceBackend::WindowsTask => {
                native::account(ops, backend, &spec.descriptor.service_group).await?;
            }
            ServiceBackend::Unmanaged => unreachable!(),
        }
    }
    let group = (!spec.descriptor.service_group.is_empty())
        .then_some(spec.descriptor.service_group.as_str());
    ops.create_dir(&spec.install_root, 0o755, None).await?;
    ops.create_dir(
        &spec.install_root.join(&spec.descriptor.artifact_name),
        0o755,
        None,
    )
    .await?;
    ops.create_dir(&spec.runtime_root, 0o750, group).await?;
    let runtime = spec
        .runtime_root
        .join(format!("{}@main", spec.descriptor.artifact_name));
    ops.create_dir(&runtime, 0o750, group).await?;
    ops.create_dir(&runtime.join("revisions"), 0o750, group)
        .await?;
    ops.create_dir(&runtime.join("data"), 0o770, group).await?;

    match backend {
        ServiceBackend::Systemd => {
            let unit_name = spec.descriptor.systemd_unit_definition_name();
            let etc_unit = Path::new("/etc/systemd/system").join(&unit_name);
            let lib_unit = Path::new("/lib/systemd/system").join(&unit_name);
            let usr_unit = Path::new("/usr/lib/systemd/system").join(&unit_name);
            if !etc_unit.exists() && !lib_unit.exists() && !usr_unit.exists() {
                let journal_conf = Path::new("/etc/systemd/journald@apsp-runtime.conf");
                if !journal_conf.exists() {
                    ops.write_file(
                        journal_conf,
                        apsp_adapter_sdk::RUNTIME_JOURNALD_CONF.as_bytes(),
                        0o644,
                        None,
                    )
                    .await?;
                }
                let unit_text = spec.descriptor.systemd_unit_contents(spec);
                ops.write_file(&etc_unit, unit_text.as_bytes(), 0o644, None)
                    .await?;
                native::command(ops, "systemctl", &["daemon-reload".into()]).await?;
                native::command(
                    ops,
                    "systemctl",
                    &[
                        "enable".into(),
                        "--".into(),
                        spec.descriptor.service_unit.clone(),
                    ],
                )
                .await?;
            }
        }
        ServiceBackend::OpenRc => {
            let service_name = spec.descriptor.openrc_service_name();
            let init_script = Path::new("/etc/init.d").join(service_name);
            if !init_script.exists() {
                let script_text = spec.descriptor.openrc_script_contents(spec);
                ops.write_file(&init_script, script_text.as_bytes(), 0o755, None)
                    .await?;
                let _ = ops.execute(Path::new("rc-update"), &["-u".into()]).await;
                native::command(
                    ops,
                    "rc-update",
                    &["add".into(), service_name.into(), "default".into()],
                )
                .await?;
            }
        }
        ServiceBackend::Launchd | ServiceBackend::FreeBsd | ServiceBackend::WindowsTask => {
            native::ensure_runtime(ops, backend, spec).await?;
        }
        ServiceBackend::Unmanaged => unreachable!(),
    }
    Ok(())
}

pub async fn install_services(
    config: &Config,
    config_path: &Path,
    descriptor: Option<&Descriptor>,
    agent_version: &str,
) -> Result<()> {
    ensure!(
        apsp_protocol::release_version(agent_version).is_some(),
        "invalid Agent release version"
    );
    if descriptor.is_none() {
        crate::retirement::ensure_monitor_only_allowed(config)?;
    }
    let backend = ServiceBackend::detect()?;
    ensure!(
        matches!(
            backend,
            ServiceBackend::Launchd | ServiceBackend::FreeBsd | ServiceBackend::WindowsTask
        ),
        "Linux services are installed through install.sh"
    );
    // Reject untrusted executables and existing runtime state before changing
    // accounts, service definitions, or the currently selected Agent.
    let source_binary = std::env::current_exe()?;
    crate::artifacts::verify_installed_binary(&source_binary, "agent", "raw").await?;
    crate::artifacts::verify_cache(config).await?;
    let proof_directory = source_binary
        .parent()
        .context("Agent binary has no parent")?;
    let mut proof_files = Vec::new();
    for name in ["release.json", "SHA256SUMS", "SHA256SUMS.minisig"] {
        proof_files.push((name, tokio::fs::read(proof_directory.join(name)).await?));
    }
    let ops: Arc<dyn Privileged> = Arc::new(SystemOps);
    native::require_admin(ops.as_ref(), backend).await?;
    if let Some(descriptor) = descriptor {
        native::account(ops.as_ref(), backend, &descriptor.service_group).await?;
    }
    let root = core_root(config)?;
    ops.create_dir(&root, 0o755, None).await?;
    ops.create_dir(
        config.state_db.parent().context("state has no parent")?,
        0o700,
        None,
    )
    .await?;
    let directory = root.join(agent_version);
    let binary = directory.join(executable_name());
    ops.create_dir(&directory, 0o755, None).await?;
    let bytes = tokio::fs::read(&source_binary).await?;
    if binary.try_exists()? {
        ensure!(
            tokio::fs::read(&binary).await? == bytes,
            "installed Agent version has different bytes"
        );
    } else {
        ops.write_file(&binary, &bytes, 0o755, None).await?;
    }
    for (name, proof) in proof_files {
        let proof_path = directory.join(name);
        if proof_path.try_exists()? {
            ensure!(
                tokio::fs::read(&proof_path).await? == proof,
                "installed Agent proof contains different bytes"
            );
        } else {
            ops.write_file(&proof_path, &proof, 0o644, None).await?;
        }
    }
    crate::artifacts::verify_installed_binary(&binary, "agent", "raw").await?;
    if let Some(descriptor) = descriptor {
        ops.create_dir(&config.install_root, 0o755, None).await?;
        ops.create_dir(&config.runtime_root, 0o750, Some(&descriptor.service_group))
            .await?;
        let runtime = config
            .runtime_root
            .join(format!("{}@main", descriptor.artifact_name));
        ops.create_dir(&runtime, 0o750, Some(&descriptor.service_group))
            .await?;
        ops.create_dir(
            &runtime.join("revisions"),
            0o750,
            Some(&descriptor.service_group),
        )
        .await?;
        ops.create_dir(
            &runtime.join("data"),
            0o770,
            Some(&descriptor.service_group),
        )
        .await?;
    }
    let previous = reference(&root.join("current")).ok();
    let services = SystemServiceManager::new(ops.clone(), backend);
    let activate = async {
        let _ = services.stop("apsp-agent.service").await;
        ops.atomic_symlink(&root.join("current"), &directory)
            .await?;
        native::register(
            ops.as_ref(),
            backend,
            config,
            config_path,
            &binary,
            descriptor,
        )
        .await?;
        services.restart("apsp-agent.service").await?;
        for _ in 0..30 {
            if let Ok(status) = crate::transport::status(&config.status_socket).await {
                if let Some(error) = status
                    .get("bundle_error")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                {
                    anyhow::bail!("Agent failed bundle startup check: {error}");
                }
                if status["agent_version"] == agent_version
                    && status.get("bundle_ready").and_then(|value| value.as_bool()) == Some(true)
                {
                    return Ok::<_, anyhow::Error>(());
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        anyhow::bail!("Agent did not pass the installation startup check")
    }
    .await;
    if let Err(error) = activate {
        let _ = services.stop("apsp-agent.service").await;
        if let Some(previous) = previous {
            ops.atomic_symlink(&root.join("current"), &previous).await?;
            native::register(
                ops.as_ref(),
                backend,
                config,
                config_path,
                &previous.join(executable_name()),
                descriptor,
            )
            .await?;
            services.restart("apsp-agent.service").await?;
        } else {
            ops.remove_symlink(&root.join("current")).await?;
        }
        return Err(error);
    }
    Ok(())
}

pub fn reference(path: &Path) -> Result<PathBuf> {
    #[cfg(unix)]
    let target = std::fs::read_link(path)?;
    #[cfg(windows)]
    let target = crate::system::read_reference(path)?;
    Ok(if target.is_absolute() {
        target
    } else {
        path.parent()
            .context("reference has no parent")?
            .join(target)
    })
}

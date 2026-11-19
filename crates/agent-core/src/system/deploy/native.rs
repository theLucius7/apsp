#[path = "native/unix.rs"]
mod unix;
#[path = "native/windows.rs"]
mod windows;
use super::*;

pub(super) async fn command(ops: &dyn Privileged, program: &str, args: &[String]) -> Result<()> {
    let output = ops.execute(Path::new(program), args).await?;
    ensure!(
        output.success,
        "service installation command failed: {}",
        output.stderr
    );
    Ok(())
}

pub(super) async fn require_admin(ops: &dyn Privileged, backend: ServiceBackend) -> Result<()> {
    if backend == ServiceBackend::WindowsTask {
        windows::powershell(ops,"if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run as Administrator' }").await
    } else {
        let output = ops.execute(Path::new("id"), &["-u".into()]).await?;
        ensure!(
            output.success && output.stdout.trim() == "0",
            "service installation requires root"
        );
        Ok(())
    }
}

pub(super) async fn account(
    ops: &dyn Privileged,
    backend: ServiceBackend,
    name: &str,
) -> Result<()> {
    ensure!(
        crate::artifacts::safe_component(name),
        "invalid service account"
    );
    match backend {
        ServiceBackend::Launchd => unix::mac_account(ops, name).await,
        ServiceBackend::FreeBsd => unix::bsd_account(ops, name).await,
        ServiceBackend::WindowsTask => windows::account(ops, name).await,
        _ => anyhow::bail!("unsupported native service backend"),
    }
}

pub(super) async fn register(
    ops: &dyn Privileged,
    backend: ServiceBackend,
    config: &Config,
    path: &Path,
    binary: &Path,
    descriptor: Option<&Descriptor>,
) -> Result<()> {
    match backend {
        ServiceBackend::WindowsTask => {
            windows::register(ops, config, path, binary, descriptor).await
        }
        ServiceBackend::FreeBsd | ServiceBackend::Launchd => {
            unix::register(ops, backend, config, path, binary, descriptor).await
        }
        _ => anyhow::bail!("unsupported native service backend"),
    }
}

pub(super) async fn ensure_runtime(
    ops: &dyn Privileged,
    backend: ServiceBackend,
    spec: &apsp_adapter_sdk::RuntimeServiceSpec,
) -> Result<()> {
    match backend {
        ServiceBackend::WindowsTask => windows::ensure_runtime(ops, spec).await,
        ServiceBackend::FreeBsd | ServiceBackend::Launchd => {
            unix::ensure_runtime(ops, backend, spec).await
        }
        _ => anyhow::bail!("unsupported native service backend"),
    }
}

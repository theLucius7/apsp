//! Runtime services can start before the supervisor confirms the new core/current.
//! Ask the incumbent to authenticate the versioned verifier before invoking it.
use super::*;
use apsp_adapter_sdk::RuntimeServiceSpec;
use apsp_protocol::bundled::BUNDLED_MARKER;

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

pub(super) async fn configure(
    ops: &dyn Privileged,
    backend: ServiceBackend,
    spec: &RuntimeServiceSpec,
) -> Result<()> {
    let root = &spec.agent_root;
    let Some((state, version)) = crate::upgrade::installed_trial(root)? else {
        return Ok(());
    };
    let candidate = root.join(version).join(executable_name());
    let previous = root.join(state.current).join(executable_name());
    let current = root.join("current").join(executable_name());
    let component = spec
        .install_root
        .join(&spec.descriptor.artifact_name)
        .join("current");
    if backend == ServiceBackend::WindowsTask {
        let ps = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "''"));
        let shim = root.join("verify-runtime.ps1");
        let script = format!(
            r#"param($active, $binary, $component)
$ErrorActionPreference='Stop'
& $active verify-installed --binary $binary --name $component --format tar.gz
if ($LASTEXITCODE -eq 0) {{ exit 0 }}
if ($active -ne {} -or !(Test-Path -LiteralPath (Join-Path (Split-Path $binary) '{}'))) {{ exit 1 }}
$candidate={}
& $active verify-installed --binary $candidate --name agent --format raw
if ($LASTEXITCODE -ne 0) {{ exit 1 }}
& $candidate verify-installed --binary $binary --name $component --format tar.gz
exit $LASTEXITCODE
"#,
            ps(&previous),
            BUNDLED_MARKER,
            ps(&candidate)
        );
        ops.write_file(&shim, script.as_bytes(), 0o755, None)
            .await?;
        let name = &spec.descriptor.artifact_name;
        let anchor = format!(
            "& $verifier verify-installed --binary $binary --name '{name}' --format tar.gz"
        );
        let replacement = format!("& {} $verifier $binary '{}'", ps(&shim), name);
        return replace_command(
            ops,
            &root.join("runtime-launcher.ps1"),
            &anchor,
            &replacement,
            0o755,
        )
        .await
        .map(|_| ());
    }

    let shim = root.join("verify-runtime");
    let script = format!(
        "#!/bin/sh\nset -eu\ncurrent={}\ncandidate={}\nif [ \"$current\" -ef {} ] && [ -f {} ]; then\n    if \"$current\" \"$@\"; then exit 0; fi\n    \"$current\" verify-installed --binary \"$candidate\" --name agent --format raw\n    exec \"$candidate\" \"$@\"\nfi\nexec \"$current\" \"$@\"\n",
        quote(&current),
        quote(&candidate),
        quote(&previous),
        quote(&component.join(BUNDLED_MARKER))
    );
    ops.write_file(&shim, script.as_bytes(), 0o755, None)
        .await?;
    let binary = component.join(&spec.descriptor.binary_name);
    let (path, old, replacement, mode) = match backend {
        ServiceBackend::Systemd => (
            Path::new("/etc/systemd/system").join(spec.descriptor.systemd_unit_definition_name()),
            format!(
                "ExecStartPre={} verify-installed --binary {}",
                current.display(),
                binary.display()
            ),
            format!(
                "ExecStartPre={} verify-installed --binary {}",
                shim.display(),
                binary.display()
            ),
            0o644,
        ),
        ServiceBackend::OpenRc => (
            Path::new("/etc/init.d").join(spec.descriptor.openrc_service_name()),
            format!(
                "{} verify-installed --binary {}",
                current.display(),
                binary.display()
            ),
            format!(
                "{} verify-installed --binary {}",
                shim.display(),
                binary.display()
            ),
            0o755,
        ),
        ServiceBackend::Launchd | ServiceBackend::FreeBsd => (
            root.join("runtime-launcher.sh"),
            format!(
                "{} verify-installed --binary {}",
                quote(&current),
                quote(&binary)
            ),
            format!(
                "{} verify-installed --binary {}",
                quote(&shim),
                quote(&binary)
            ),
            0o755,
        ),
        _ => anyhow::bail!("unsupported managed runtime verifier"),
    };
    if replace_command(ops, &path, &old, &replacement, mode).await?
        && backend == ServiceBackend::Systemd
    {
        native::command(ops, "systemctl", &["daemon-reload".into()]).await?;
    }
    Ok(())
}

async fn replace_command(
    ops: &dyn Privileged,
    path: &Path,
    old: &str,
    replacement: &str,
    mode: u32,
) -> Result<bool> {
    let original = std::fs::read_to_string(path).context("read managed runtime service")?;
    if original.contains(replacement) {
        return Ok(false);
    }
    ensure!(
        original.contains(old),
        "runtime service has no managed verification step"
    );
    ops.write_file(
        path,
        original.replace(old, replacement).as_bytes(),
        mode,
        None,
    )
    .await?;
    Ok(true)
}

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
        let launcher = root.join("runtime-launcher.ps1");
        return replace_command(ops, &launcher, &launcher, &anchor, &replacement, 0o755).await;
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
    let source = if backend == ServiceBackend::Systemd {
        let name = spec.descriptor.systemd_unit_definition_name();
        super::systemd_definition(
            &path,
            &Path::new("/lib/systemd/system").join(&name),
            &Path::new("/usr/lib/systemd/system").join(&name),
        )?
        .context("managed runtime service is missing")?
    } else {
        path.clone()
    };
    replace_command(ops, &source, &path, &old, &replacement, mode).await?;
    if backend == ServiceBackend::Systemd {
        // A prior worker may have stopped between the atomic write and reload.
        native::command(ops, "systemctl", &["daemon-reload".into()]).await?;
    }
    Ok(())
}

async fn replace_command(
    ops: &dyn Privileged,
    source: &Path,
    path: &Path,
    old: &str,
    replacement: &str,
    mode: u32,
) -> Result<()> {
    let original = std::fs::read_to_string(source).context("read managed runtime service")?;
    let mut matched = false;
    let mut updated = String::with_capacity(original.len());
    for line in original.split_inclusive('\n') {
        let command = line.trim_start_matches([' ', '\t']);
        let is_prefix = |prefix: &str| {
            command
                .strip_prefix(prefix)
                .is_some_and(|tail| tail.is_empty() || tail.starts_with(char::is_whitespace))
        };
        if is_prefix(old) {
            matched = true;
            updated.push_str(&line.replacen(old, replacement, 1));
        } else {
            matched |= is_prefix(replacement);
            updated.push_str(line);
        }
    }
    ensure!(matched, "runtime service has no managed verification step");
    if source != path || updated != original {
        ops.write_file(path, updated.as_bytes(), mode, None).await?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::MetadataExt};
    use uuid::Uuid;

    #[tokio::test]
    async fn trial_overrides_vendor_unit_without_editing_vendor_bytes() {
        let dir = std::env::temp_dir().join(format!("apsp-vendor-unit-{}", Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let local = dir.join("etc-unit");
        let lib = dir.join("lib-unit");
        let usr_lib = dir.join("usr-unit");
        let old = "ExecStartPre=/current verify-installed --binary /runtime";
        let new = "ExecStartPre=/shim verify-installed --binary /runtime";
        let content = format!("# {old}\r\n[Service]\r\n{old} --name demo\r\nExecStart=/runtime");
        fs::write(&usr_lib, &content).unwrap();
        assert_eq!(
            systemd_definition(&local, &lib, &usr_lib).unwrap(),
            Some(usr_lib.clone())
        );
        let expected = content.replace(&format!("\n{old}"), &format!("\n{new}"));
        for _ in 0..2 {
            let source = systemd_definition(&local, &lib, &usr_lib).unwrap().unwrap();
            replace_command(&SystemOps, &source, &local, old, new, 0o644)
                .await
                .unwrap();
            assert_eq!(fs::read_to_string(&local).unwrap(), expected);
            assert_eq!(fs::read_to_string(&usr_lib).unwrap(), content);
            assert_eq!(
                systemd_definition(&local, &lib, &usr_lib).unwrap(),
                Some(local.clone())
            );
            let metadata = fs::metadata(&local).unwrap();
            assert_eq!(metadata.mode() & 0o7777, 0o644);
            assert_eq!(metadata.uid(), fs::metadata(&dir).unwrap().uid());
        }
        fs::remove_dir_all(dir).unwrap();
    }
}

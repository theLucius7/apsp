pub use apsp_adapter_sdk::{Privileged, ServiceManager};
#[path = "command_process.rs"]
mod command_process;
#[path = "execution.rs"]
mod execution;
#[path = "publication.rs"]
mod publication;
#[path = "services.rs"]
mod services;
pub use services::{ServiceBackend, SystemServiceManager};
#[path = "archive.rs"]
mod archive;
#[path = "deploy.rs"]
pub mod deploy;

use crate::artifacts::safe_component;
use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::{BoxFuture, CommandOutput, Execution, JobStatus, ServiceJob};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;
use uuid::Uuid;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
#[derive(Default)]
pub struct SystemOps;

pub fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

impl SystemOps {
    pub async fn protect(&self, path: &Path, mode: u32, group: Option<&str>) -> Result<()> {
        self.protect_many(&[(path, mode, group)]).await
    }

    async fn protect_many(&self, paths: &[(&Path, u32, Option<&str>)]) -> Result<()> {
        let mut entries = Vec::new();
        for (path, mode, group) in paths {
            if let Some(group) = group {
                ensure!(safe_component(group), "invalid runtime account");
            }
            entries.push(format!(
                "@{{Path={};Mode={mode};Group={}}}",
                ps_quote(&path.to_string_lossy()),
                group.map(ps_quote).unwrap_or_else(|| "$null".into())
            ));
        }
        // Use .NET directly: loading the filesystem/security cmdlet modules for
        // every path is expensive on native ARM64 Windows PowerShell.
        let script = format!(
            r#"$ErrorActionPreference='Stop'
$owner=[Security.Principal.WindowsIdentity]::GetCurrent().User
function Add-Rule($sid,$rights) {{
    $rule=[Security.AccessControl.FileSystemAccessRule]::new($sid,[Security.AccessControl.FileSystemRights]$rights,$inherit,[Security.AccessControl.PropagationFlags]::None,[Security.AccessControl.AccessControlType]::Allow)
    $acl.AddAccessRule($rule)
}}
foreach($entry in @({})) {{
    $p=$entry.Path
    $attributes=[IO.File]::GetAttributes($p)
    if($attributes -band [IO.FileAttributes]::ReparsePoint) {{ throw "Reparse points are not allowed: $p" }}
    $directory=($attributes -band [IO.FileAttributes]::Directory) -ne 0
    $acl=if($directory) {{ [Security.AccessControl.DirectorySecurity]::new() }} else {{ [Security.AccessControl.FileSecurity]::new() }}
    $acl.SetAccessRuleProtection($true,$false)
    $inherit=if($directory) {{ [Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit' }} else {{ [Security.AccessControl.InheritanceFlags]::None }}
    $acl.SetOwner($owner)
    Add-Rule $owner 'FullControl'
    Add-Rule ([Security.Principal.SecurityIdentifier]'S-1-5-18') 'FullControl'
    Add-Rule ([Security.Principal.SecurityIdentifier]'S-1-5-32-544') 'FullControl'
    if($entry.Group) {{
        $sid=([Security.Principal.NTAccount]::new($entry.Group)).Translate([Security.Principal.SecurityIdentifier])
        $rights=if($entry.Mode -band 16) {{ 'Modify' }} else {{ 'ReadAndExecute' }}
        Add-Rule $sid $rights
    }}
    if($entry.Mode -band 4) {{ Add-Rule ([Security.Principal.SecurityIdentifier]'S-1-5-32-545') 'ReadAndExecute' }}
    if($directory) {{ [IO.Directory]::SetAccessControl($p,$acl) }} else {{ [IO.File]::SetAccessControl($p,$acl) }}
}}
"#,
            entries.join(",")
        );
        let output = self
            .execute_bounded(
                Path::new("powershell.exe"),
                &[
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    script,
                ],
                90,
                1024 * 1024,
            )
            .await
            .context("protect Windows paths")?;
        ensure!(
            !output.timed_out,
            "protecting Windows paths exceeded 90 seconds"
        );
        ensure!(
            output.output.success,
            "cannot protect Windows state directory: {}",
            output.output.stderr
        );
        Ok(())
    }
}

pub fn check_private(path: &Path) -> Result<()> {
    let kind = if path.is_dir() { "Directory" } else { "File" };
    let script = format!(
        r#"$ErrorActionPreference='Stop'; $allowed=@([Security.Principal.WindowsIdentity]::GetCurrent().User.Value,'S-1-5-18','S-1-5-32-544'); $acl=[IO.{kind}]::GetAccessControl({}); $allowed+=$acl.GetOwner([Security.Principal.SecurityIdentifier]).Value; foreach($rule in $acl.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier])) {{ if($rule.AccessControlType -eq 'Allow' -and $allowed -notcontains $rule.IdentityReference.Value) {{ throw 'State grants access to another account' }} }}"#,
        ps_quote(&path.to_string_lossy())
    );
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()?;
    ensure!(
        output.status.success(),
        "device identity or status file is not private"
    );
    Ok(())
}

impl Privileged for SystemOps {
    fn execute_controlled<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
        seconds: u32,
        maximum: usize,
        observer: &'a dyn apsp_adapter_sdk::CommandObserver,
    ) -> BoxFuture<'a, apsp_adapter_sdk::ControlledExecution> {
        Box::pin(command_process::execute(
            program, args, seconds, maximum, observer,
        ))
    }
    fn recover_command<'a>(
        &'a self,
        process: &'a apsp_adapter_sdk::CommandProcessIdentity,
    ) -> BoxFuture<'a, ()> {
        Box::pin(command_process::recover(process))
    }
    fn spawn_managed<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
    ) -> BoxFuture<'a, Box<dyn apsp_adapter_sdk::ManagedProcess>> {
        Box::pin(async move { execution::spawn(program, args) })
    }
    fn execute<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
    ) -> BoxFuture<'a, CommandOutput> {
        Box::pin(async move {
            if publication::is_request(program, args) {
                publication::validate(args)?;
                return publication::simulate(args).await;
            }
            let output = execution::execute(program, args, 30, 1024 * 1024).await?;
            ensure!(!output.timed_out, "command exceeded 30 seconds");
            Ok(output.output)
        })
    }
    fn execute_bounded<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
        seconds: u32,
        maximum: usize,
    ) -> BoxFuture<'a, Execution> {
        Box::pin(execution::execute(program, args, seconds, maximum))
    }
    fn create_dir<'a>(
        &'a self,
        path: &'a Path,
        mode: u32,
        group: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut missing = Vec::new();
            let mut current = path;
            while !current.exists() {
                missing.push(current.to_owned());
                current = current
                    .parent()
                    .context("directory has no existing ancestor")?;
            }
            for directory in missing.iter().rev() {
                fs::create_dir(directory)?;
            }
            let mut permissions: Vec<_> = missing
                .iter()
                .rev()
                .filter(|directory| directory.as_path() != path)
                .map(|directory| (directory.as_path(), 0o755, None))
                .collect();
            permissions.push((path, mode, group));
            self.protect_many(&permissions).await
        })
    }
    fn write_file<'a>(
        &'a self,
        path: &'a Path,
        bytes: &'a [u8],
        mode: u32,
        group: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let parent = path.parent().context("file has no parent")?;
            if !parent.exists() {
                self.create_dir(parent, 0o700, None).await?;
            }
            let temporary = parent.join(format!(".write-{}", Uuid::new_v4()));
            let result = async {
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)?;
                self.protect(&temporary, mode, group).await?;
                file.write_all(bytes)?;
                file.sync_all()?;
                drop(file);
                fs::rename(&temporary, path)?;
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if result.is_err() {
                let _ = fs::remove_file(temporary);
            }
            result
        })
    }
    fn atomic_symlink<'a>(&'a self, link: &'a Path, target: &'a Path) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if link.exists() {
                read_reference(link)?;
            }
            self.write_file(
                link,
                &serde_json::to_vec(
                    &serde_json::json!({"apsp_directory_reference":true,"target":target}),
                )?,
                0o644,
                None,
            )
            .await
        })
    }
    fn remove_symlink<'a>(&'a self, link: &'a Path) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if link.exists() {
                read_reference(link)?;
                fs::remove_file(link)?;
            }
            Ok(())
        })
    }
    fn remove_path<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let metadata = match fs::symlink_metadata(path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            };
            remove_managed(path, metadata.is_dir())
        })
    }
    fn remove_file<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, ()> {
        Box::pin(async move { remove_managed(path, false) })
    }
    fn remove_managed_directory<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, ()> {
        Box::pin(async move { remove_managed(path, true) })
    }
    fn install_archive<'a>(
        &'a self,
        path: &'a Path,
        directory: &'a Path,
        binary_name: &'a str,
    ) -> BoxFuture<'a, ()> {
        self.install_archive_files(path, directory, binary_name, &[])
    }
    fn install_archive_files<'a>(
        &'a self,
        path: &'a Path,
        directory: &'a Path,
        binary_name: &'a str,
        extras: &'a [String],
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.create_dir(
                directory.parent().context("artifact has no parent")?,
                0o755,
                None,
            )
            .await?;
            let (source, destination, name, files) = (
                path.to_owned(),
                directory.to_owned(),
                binary_name.to_owned(),
                extras.to_vec(),
            );
            tokio::task::spawn_blocking(move || {
                archive::install(&source, &destination, &name, &files)
            })
            .await??;
            let paths: Vec<_> = std::iter::once(binary_name)
                .chain(extras.iter().map(String::as_str))
                .map(|name| directory.join(name))
                .chain(std::iter::once(directory.to_owned()))
                .collect();
            let permissions: Vec<_> = paths
                .iter()
                .map(|path| (path.as_path(), 0o755, None))
                .collect();
            self.protect_many(&permissions).await
        })
    }
}

pub fn read_reference(path: &Path) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 16 * 1024,
        "invalid directory reference"
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
    ensure!(
        value["apsp_directory_reference"] == true,
        "refusing to replace an ordinary file"
    );
    Ok(PathBuf::from(
        value["target"]
            .as_str()
            .context("reference has no target")?,
    ))
}

impl SystemServiceManager {
    pub(super) fn diagnostic_running_units(&self) -> BoxFuture<'_, Vec<String>> {
        Box::pin(async { anyhow::bail!("安全诊断仅支持 Linux 服务后端") })
    }
    fn start_diagnostic_job<'a>(&'a self, _job: &'a ServiceJob) -> BoxFuture<'a, ()> {
        Box::pin(async { anyhow::bail!("diagnostic jobs require Linux") })
    }
    fn diagnostic_job_status<'a>(&'a self, _unit: &'a str) -> BoxFuture<'a, JobStatus> {
        Box::pin(async { anyhow::bail!("diagnostic jobs require Linux") })
    }
}

fn parent_directory(path: &Path) -> Result<&Path> {
    let parent = path.parent().context("path has no parent")?;
    ensure!(
        !parent.as_os_str().is_empty(),
        "path must have an explicit parent"
    );
    Ok(parent)
}

fn sync_directory(_path: &Path) -> Result<()> {
    // std does not expose Windows directory handles suitable for FlushFileBuffers.
    // Every payload and proof is synchronized before the directory is published.
    Ok(())
}

fn remove_managed(path: &Path, directory: bool) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    ensure!(
        path.is_absolute(),
        "managed removal requires an absolute path"
    );
    let parent = parent_directory(path)?;
    for ancestor in parent.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)?;
        ensure!(
            metadata.is_dir() && metadata.file_attributes() & 0x400 == 0,
            "managed removal refuses reparse point ancestors"
        );
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.file_attributes() & 0x400 == 0,
        "managed removal refuses reparse points"
    );
    if directory {
        ensure!(metadata.is_dir(), "managed removal requires a directory");
        fs::remove_dir_all(path)?;
    } else {
        ensure!(
            metadata.is_file(),
            "credential removal requires a regular file"
        );
        fs::remove_file(path)?;
    }
    Ok(())
}

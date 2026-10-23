use super::*;
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
pub(super) async fn powershell(ops: &dyn Privileged, script: &str) -> Result<()> {
    let result = ops
        .execute_bounded(
            Path::new("powershell.exe"),
            &[
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                format!("$ErrorActionPreference='Stop'; {script}"),
            ],
            120,
            1024 * 1024,
        )
        .await?;
    ensure!(
        !result.timed_out,
        "native service setup exceeded 120 seconds"
    );
    ensure!(
        result.output.success,
        "native service setup failed: {}",
        result.output.stderr
    );
    Ok(())
}

pub(super) async fn account(ops: &dyn Privileged, name: &str) -> Result<()> {
    let script = format!(
        r#"$name={}
$account=Get-LocalUser -Name $name -ErrorAction SilentlyContinue
if(-not $account) {{ $account=New-LocalUser -Name $name -Disabled -NoPassword -AccountNeverExpires -UserMayNotChangePassword }}
if(-not (Get-LocalGroupMember -SID 'S-1-5-32-545' | Where-Object {{ $_.SID -eq $account.SID }})) {{ Add-LocalGroupMember -SID 'S-1-5-32-545' -Member $account }}
$temporary=[IO.Path]::Combine([IO.Path]::GetTempPath(),'apsp-policy-'+[Guid]::NewGuid().ToString('N'))
$owner=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$acl=[Security.AccessControl.DirectorySecurity]::new()
$acl.SetSecurityDescriptorSddlForm("D:P(A;OICI;FA;;;$owner)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)")
$null=[IO.Directory]::CreateDirectory($temporary,$acl)
function Normalize-Principal($principal) {{
    $value=$principal.Trim()
    try {{
        if($value.TrimStart('*') -match '^S-[0-9]+-') {{
            return '*'+([Security.Principal.SecurityIdentifier]::new($value.TrimStart('*'))).Value
        }}
        return '*'+([Security.Principal.NTAccount]::new($value)).Translate([Security.Principal.SecurityIdentifier]).Value
    }} catch {{
        # Preserve principals that are temporarily unavailable, including domain accounts.
        return $value
    }}
}}
try {{
    $export=[IO.Path]::Combine($temporary,'existing.inf')
    $template=[IO.Path]::Combine($temporary,'grant.inf')
    $database=[IO.Path]::Combine($temporary,'policy.sdb')
    $log=[IO.Path]::Combine($temporary,'policy.log')
    & secedit.exe /export /cfg $export /areas USER_RIGHTS /log $log /quiet
    if($LASTEXITCODE -ne 0) {{ throw 'Cannot read local batch logon rights' }}
    $rights=@()
    foreach($line in [IO.File]::ReadAllLines($export)) {{
        if($line -match '^\s*SeBatchLogonRight\s*=(.*)$') {{
            foreach($member in $Matches[1].Split(',')) {{ if($member.Trim()) {{ $rights+=$member.Trim() }} }}
        }}
    }}
    $sid='*'+$account.SID.Value
    $identities=@($rights | ForEach-Object {{ Normalize-Principal $_ }})
    foreach($line in [IO.File]::ReadAllLines($export)) {{
        if($line -match '^\s*SeDenyBatchLogonRight\s*=(.*)$') {{
            $denied=@($Matches[1].Split(',') | ForEach-Object {{ Normalize-Principal $_ }})
            if(@($sid,'*S-1-1-0','*S-1-5-11','*S-1-5-32-545') | Where-Object {{ $denied -contains $_ }}) {{
                throw 'Existing policy denies runtime batch logon; no denial was changed'
            }}
        }}
    }}
    if($identities -notcontains $sid) {{
        $rights+=$sid
        $lines=@('[Unicode]','Unicode=yes','[Version]','signature="$CHICAGO$"','Revision=1','[Privilege Rights]',('SeBatchLogonRight = '+($rights -join ',')))
        [IO.File]::WriteAllLines($template,$lines,[Text.Encoding]::Unicode)
        & secedit.exe /configure /db $database /cfg $template /areas USER_RIGHTS /log $log /quiet
        if($LASTEXITCODE -ne 0) {{ throw 'Cannot grant runtime batch logon rights' }}
    }}
    & secedit.exe /export /cfg $export /areas USER_RIGHTS /log $log /quiet
    if($LASTEXITCODE -ne 0) {{ throw 'Cannot verify runtime batch logon rights' }}
    $verified=@()
    foreach($line in [IO.File]::ReadAllLines($export)) {{
        if($line -match '^\s*SeBatchLogonRight\s*=(.*)$') {{
            $verified=@($Matches[1].Split(',') | ForEach-Object {{ Normalize-Principal $_ }})
        }}
    }}
    foreach($member in $rights) {{ if($verified -notcontains (Normalize-Principal $member)) {{ throw 'Batch logon grant was not preserved' }} }}
}} finally {{ [IO.Directory]::Delete($temporary,$true) }}
"#,
        quote(name)
    );
    powershell(ops, &script).await
}

async fn task(
    ops: &dyn Privileged,
    name: &str,
    program: &Path,
    args: &str,
    account: Option<&str>,
    directory: &Path,
) -> Result<()> {
    let principal = if let Some(account) = account {
        format!(
            "$random=New-Object byte[] 48; [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($random); $password='Aa1!'+[Convert]::ToBase64String($random); Set-LocalUser -Name {} -Password (ConvertTo-SecureString $password -AsPlainText -Force) -PasswordNeverExpires $true; Enable-LocalUser -Name {}; Register-ScheduledTask -TaskName {} -Action $action -Trigger $trigger -Settings $settings -User {} -Password $password -RunLevel Limited -Force | Out-Null",
            quote(account),
            quote(account),
            quote(name),
            quote(account)
        )
    } else {
        format!(
            "$principal=New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest; Register-ScheduledTask -TaskName {} -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Force | Out-Null",
            quote(name)
        )
    };
    let script = format!(
        "$action=New-ScheduledTaskAction -Execute {} -Argument {} -WorkingDirectory {}; $trigger=New-ScheduledTaskTrigger -AtStartup; $settings=New-ScheduledTaskSettingsSet -StartWhenAvailable -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries; {principal}; $scheduler=New-Object -ComObject Schedule.Service; $scheduler.Connect(); $registered=$scheduler.GetFolder('\\').GetTask({}); $registered.SetSecurityDescriptor('D:P(A;;GA;;;SY)(A;;GA;;;BA)',0x10)",
        quote(&program.to_string_lossy()),
        quote(args),
        quote(&directory.to_string_lossy()),
        quote(name)
    );
    powershell(ops, &script).await
}

pub(super) async fn register(
    ops: &dyn Privileged,
    config: &Config,
    path: &Path,
    binary: &Path,
    descriptor: Option<&Descriptor>,
) -> Result<()> {
    let root = core_root(config)?;
    ensure!(
        !path.to_string_lossy().contains('"'),
        "configuration path cannot contain a quote"
    );
    let args = format!(
        "--config \"{}\" supervise{}",
        path.display(),
        if descriptor.is_none() {
            " --monitor-only"
        } else {
            ""
        }
    );
    task(ops, "apsp-agent", binary, &args, None, &root).await?;
    if let Some(descriptor) = descriptor {
        let runtime = config
            .runtime_root
            .join(format!("{}@main", descriptor.plugin_name));
        let script = root.join("runtime-launcher.ps1");
        let contents = runtime_launcher(
            &root.join("current"),
            &config
                .install_root
                .join(&descriptor.plugin_name)
                .join("current"),
            &runtime.join("current"),
            &runtime.join("data"),
            &descriptor.binary_name,
            &descriptor.plugin_name,
        );
        ops.write_file(&script, contents.as_bytes(), 0o755, None)
            .await?;
        let name = descriptor
            .service_unit
            .strip_suffix(".service")
            .unwrap_or(&descriptor.service_unit);
        task(
            ops,
            name,
            Path::new("powershell.exe"),
            &format!(
                "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\"",
                script.display()
            ),
            Some(&descriptor.service_group),
            &runtime.join("data"),
        )
        .await?;
    }
    Ok(())
}

fn runtime_launcher(
    agent_reference: &Path,
    kernel_reference: &Path,
    configuration_reference: &Path,
    data: &Path,
    binary_name: &str,
    plugin: &str,
) -> String {
    format!(
        r#"$ErrorActionPreference='Stop'
$kernel={}
$configuration={}
$agentReference={}
while ($true) {{
    if ((Test-Path -LiteralPath $kernel) -and (Test-Path -LiteralPath $configuration)) {{
        $k=Get-Content -LiteralPath $kernel -Raw | ConvertFrom-Json
        $c=Get-Content -LiteralPath $configuration -Raw | ConvertFrom-Json
        if (-not $k.apsp_directory_reference -or -not $c.apsp_directory_reference) {{ throw 'Invalid runtime reference' }}
        $binary=Join-Path $k.target {}
        $config=Join-Path $c.target 'config.json'
        $a=Get-Content -LiteralPath $agentReference -Raw | ConvertFrom-Json
        if (-not $a.apsp_directory_reference) {{ throw 'Invalid Agent reference' }}
        $verifier=Join-Path $a.target 'apsp-agent.exe'
        & $verifier verify-installed --binary $binary --name {} --format tar.gz
        if ($LASTEXITCODE -ne 0) {{ throw 'Runtime signature verification failed' }}
        & $binary run -c $config -D {}
    }}
    Start-Sleep -Seconds 5
}}
"#,
        quote(&kernel_reference.to_string_lossy()),
        quote(&configuration_reference.to_string_lossy()),
        quote(&agent_reference.to_string_lossy()),
        quote(binary_name),
        quote(plugin),
        quote(&data.to_string_lossy()),
    )
}

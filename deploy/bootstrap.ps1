# Standalone trusted bootstrap. Downloaded Agent bytes are never executed before verification.
[CmdletBinding()]
param(
    [string]$Version = 'latest',
    [Parameter(Mandatory = $true)][string]$Panel,
    [Parameter(Mandatory = $true)][string]$Token,
    [string]$Target = 'auto',
    [string]$Mirror = ''
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$script:Roots = @('RWS4aZYmyBmwROpGKjfADJqNedYCNRhlg0+UoIBjQHxXZxYL7XMlkGJN')
$script:VersionPattern = '^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$'
$script:Targets = @('amd64', 'arm64', 'linux-gnu-amd64', 'linux-gnu-arm64', 'linux-musl-amd64', 'linux-musl-arm64', 'macos-arm64', 'freebsd-amd64', 'freebsd-arm64', 'windows-amd64', 'windows-arm64')

function Assert-Apsp([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function Test-Integer($Value) {
    return $Value -is [int] -or $Value -is [long]
}
function Assert-Fields($Value, [string[]]$Expected) {
    Assert-Apsp ($null -ne $Value -and $Value -is [pscustomobject]) '发布信息必须为对象'
    $fields = @($Value.PSObject.Properties.Name)
    Assert-Apsp ($fields.Count -eq $Expected.Count -and @(Compare-Object $fields $Expected -CaseSensitive).Count -eq 0) '发布信息字段无效'
}
function Get-ApspHash([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}
function Read-ApspText([string]$Path, [long]$Limit) {
    $item = Get-Item -LiteralPath $Path -Force
    Assert-Apsp (-not $item.PSIsContainer -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0 -and $item.Length -gt 0 -and $item.Length -le $Limit) '发布文件类型或大小无效'
    return [Text.UTF8Encoding]::new($false, $true).GetString([IO.File]::ReadAllBytes($Path))
}
function Assert-Origin([string]$Value) {
    $uri = $null
    Assert-Apsp ([Uri]::TryCreate($Value, [UriKind]::Absolute, [ref]$uri) -and $Value -notmatch '[\x00-\x20\x7f]' -and $uri.Scheme -in @('https', 'http') -and $uri.Host -and -not $uri.UserInfo -and $uri.AbsolutePath -eq '/' -and -not $uri.Query -and -not $uri.Fragment) '面板地址必须是有效的 HTTPS 源地址'
    if ($uri.Scheme -eq 'http') {
        $address = $null
        $loopback = $uri.Host -eq 'localhost'
        if ([Net.IPAddress]::TryParse($uri.Host.Trim('[', ']'), [ref]$address)) {
            if ($address.IsIPv4MappedToIPv6) { $address = $address.MapToIPv4() }
            $loopback = [Net.IPAddress]::IsLoopback($address)
        }
        Assert-Apsp $loopback 'HTTP 面板仅允许回环地址，请使用 HTTPS'
    }
}
function Assert-Mirror([string]$Value, [string]$Origin) {
    if (-not $Value) { return }
    $uri = $null
    Assert-Apsp ($Value.Length -le 512 -and $Value -notmatch '[\x00-\x20\x7f]' -and [Uri]::TryCreate($Value, [UriKind]::Absolute, [ref]$uri) -and $uri.Scheme -eq 'https' -and $uri.Port -eq 443 -and $uri.Host -and -not $uri.UserInfo -and -not $uri.Query -and -not $uri.Fragment -and $uri.Host -ne 'localhost') 'Agent 下载镜像必须是独立的 HTTPS 前缀'
    $address = $null
    Assert-Apsp (-not [Net.IPAddress]::TryParse($uri.Host.Trim('[', ']'), [ref]$address)) 'Agent 下载镜像必须使用域名'
    Assert-Apsp ($uri.GetLeftPart([UriPartial]::Authority) -ine ([Uri]$Origin).GetLeftPart([UriPartial]::Authority)) 'Agent 二进制不能从面板下载'
}
function Get-ReleaseUrl([string]$Url, [string]$Prefix) {
    if ($Prefix) { return $Prefix.TrimEnd('/') + '/' + $Url }
    return $Url
}
function Assert-GithubUrl([Uri]$Uri, [string]$MirrorHost = '') {
    Assert-Apsp ($Uri.Scheme -eq 'https' -and $Uri.Port -eq 443 -and -not $Uri.UserInfo -and -not $Uri.Fragment -and ($Uri.Host -in @('github.com', 'release-assets.githubusercontent.com', 'objects.githubusercontent.com') -or ($MirrorHost -and $Uri.Host -eq $MirrorHost))) 'GitHub 下载地址超出固定允许范围'
}
function Receive-ApspFile([string]$Url, [string]$Path, [long]$Limit, [bool]$Github = $false, [long]$ExpectedSize = 0, [string]$MirrorHost = '') {
    Add-Type -AssemblyName System.Net.Http
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $handler = [Net.Http.HttpClientHandler]::new()
    $handler.UseProxy = $false
    $handler.AllowAutoRedirect = $false
    $client = [Net.Http.HttpClient]::new($handler)
    $client.Timeout = [TimeSpan]::FromSeconds(300)
    $client.DefaultRequestHeaders.UserAgent.ParseAdd('apsp-bootstrap')
    $deadline = [DateTime]::UtcNow.AddSeconds(300)
    $cancel = [Threading.CancellationTokenSource]::new([TimeSpan]::FromSeconds(300))
    $response = $null
    try {
        $uri = [Uri]$Url
        for ($redirect = 0; ; $redirect++) {
            if ($Github) { Assert-GithubUrl $uri $MirrorHost }
            $request = [Net.Http.HttpRequestMessage]::new([Net.Http.HttpMethod]::Get, $uri)
            try { $response = $client.SendAsync($request, [Net.Http.HttpCompletionOption]::ResponseHeadersRead, $cancel.Token).GetAwaiter().GetResult() }
            finally { $request.Dispose() }
            if ([int]$response.StatusCode -in @(301, 302, 303, 307, 308)) {
                Assert-Apsp ($Github -and $redirect -lt 5 -and $null -ne $response.Headers.Location) '下载禁止重定向或超过允许次数'
                $uri = [Uri]::new($uri, $response.Headers.Location)
                $response.Dispose(); $response = $null
                continue
            }
            Assert-Apsp ([int]$response.StatusCode -eq 200) ('下载失败，HTTP ' + [int]$response.StatusCode)
            break
        }
        $length = $response.Content.Headers.ContentLength
        Assert-Apsp ($null -eq $length -or ($length -gt 0 -and $length -le $Limit -and ($ExpectedSize -eq 0 -or $length -eq $ExpectedSize))) '下载长度不符合签名或大小限制'
        $inputStream = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
        $outputStream = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try {
            $buffer = [byte[]]::new(65536)
            [long]$total = 0
            while ($true) {
                Assert-Apsp ([DateTime]::UtcNow -lt $deadline) '下载超时'
                $count = $inputStream.ReadAsync($buffer, 0, $buffer.Length, $cancel.Token).GetAwaiter().GetResult()
                if ($count -eq 0) { break }
                $total += $count
                Assert-Apsp ($total -le $Limit) '下载超出大小限制'
                $outputStream.Write($buffer, 0, $count)
            }
            Assert-Apsp ($total -gt 0 -and ($ExpectedSize -eq 0 -or $total -eq $ExpectedSize)) '下载大小与已签信息不匹配'
            $outputStream.Flush($true)
        } finally { $outputStream.Dispose(); $inputStream.Dispose() }
    } catch {
        if (Test-Path -LiteralPath $Path) { Remove-Item -LiteralPath $Path -Force }
        throw
    } finally {
        if ($null -ne $response) { $response.Dispose() }
        $cancel.Dispose(); $client.Dispose(); $handler.Dispose()
    }
}
function Get-HostTarget {
    # PROCESSOR_ARCHITEW6432 preserves the OS architecture in a WOW64 shell.
    $architecture = $env:PROCESSOR_ARCHITEW6432
    if (-not $architecture) { $architecture = $env:PROCESSOR_ARCHITECTURE }
    switch ($architecture.ToUpperInvariant()) {
        'AMD64' { return 'windows-amd64' }
        'ARM64' { return 'windows-arm64' }
        default { throw 'Windows Agent 仅支持 AMD64 和 ARM64 架构' }
    }
}
function Get-Minisign([string]$Directory, [string]$Actual) {
    $zip = Join-Path $Directory 'minisign.zip'
    Receive-ApspFile 'https://github.com/jedisct1/minisign/releases/download/0.12/minisign-0.12-win64.zip' $zip 1048576 $true
    Assert-Apsp ((Get-ApspHash $zip) -eq '37b600344e20c19314b2e82813db2bfdcc408b77b876f7727889dbd46d539479') '验签工具下载校验失败'
    $architecture = if ($Actual -eq 'windows-arm64') { 'aarch64' } else { 'x86_64' }
    $digest = if ($Actual -eq 'windows-arm64') { 'f39e065e649d5ed7075675accfe0ada234175d63479df650654ec4365d7c4513' } else { '5535be9e4e123831ebe6ef324aafe9dde507015c176191f9e20c3ad60567f9e1' }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::OpenRead($zip)
    $destination = Join-Path $Directory 'minisign.exe'
    try {
        $entries = @($archive.Entries | Where-Object { $_.FullName -ceq ('minisign-win64/' + $architecture + '/minisign.exe') })
        Assert-Apsp ($entries.Count -eq 1 -and $entries[0].Length -gt 0 -and $entries[0].Length -le 1048576) '验签工具压缩包无效'
        $inputStream = $entries[0].Open()
        $outputStream = [IO.File]::Open($destination, [IO.FileMode]::CreateNew)
        try { $inputStream.CopyTo($outputStream) } finally { $inputStream.Dispose(); $outputStream.Dispose() }
    } finally { $archive.Dispose() }
    Assert-Apsp ((Get-ApspHash $destination) -eq $digest) '验签工具二进制校验失败'
    return $destination
}
function Test-ReleaseSignature([string]$Directory, [string]$Minisign) {
    $signature = Read-ApspText (Join-Path $Directory 'SHA256SUMS.minisig') 16384
    $lines = @($signature.TrimEnd("`n").Split("`n"))
    Assert-Apsp ($lines.Count -eq 4 -and $lines[0].StartsWith('untrusted comment: ') -and $lines[2].StartsWith('trusted comment: ')) '必须提供完整四行签名'
    $record = [Convert]::FromBase64String($lines[1])
    Assert-Apsp ($record.Length -eq 74 -and $record[0] -eq 69 -and $record[1] -eq 68) '禁止旧式签名'
    foreach ($key in $script:Roots) {
        $preference = $ErrorActionPreference
        try {
            $ErrorActionPreference = 'Continue'
            # A failed launch can leave a previous native exit code untouched.
            $global:LASTEXITCODE = $null
            & $Minisign -V -H -q -m (Join-Path $Directory 'SHA256SUMS') -x (Join-Path $Directory 'SHA256SUMS.minisig') -P $key >$null 2>$null
            $succeeded = $?
            $exitCode = $global:LASTEXITCODE
            Assert-Apsp ($null -ne $exitCode -and ($exitCode -ne 0 -or $succeeded)) '无法运行验签工具，拒绝执行 Agent'
            if ($succeeded -and $exitCode -eq 0) { return }
        } finally { $ErrorActionPreference = $preference }
    }
    throw '正式信任根无法验证发布签名，拒绝执行 Agent'
}
function Read-ReleaseManifest([string]$Directory, [string]$ExpectedVersion) {
    $checksums = Read-ApspText (Join-Path $Directory 'SHA256SUMS') 8192
    $rows = [Collections.Generic.Dictionary[string,string]]::new([StringComparer]::Ordinal)
    foreach ($line in $checksums.TrimEnd("`n").Split("`n")) {
        Assert-Apsp ($line -cmatch '^([0-9a-f]{64})  ([0-9A-Za-z.+_/-]+)$') '校验清单格式无效'
        $digest, $path = $Matches[1], $Matches[2]
        Assert-Apsp (-not $rows.ContainsKey($path) -and -not $path.StartsWith('/') -and $path.Split('/') -notcontains '..') '重复或不安全的校验路径'
        $rows.Add($path, $digest)
    }
    $paths = [string[]]@($rows.Keys)
    [Array]::Sort($paths, [StringComparer]::Ordinal)
    $canonical = ($paths | ForEach-Object { $rows[$_] + '  ' + $_ + "`n" }) -join ''
    Assert-Apsp ($checksums -ceq $canonical -and $rows.ContainsKey('release.json') -and $rows['release.json'] -ceq (Get-ApspHash (Join-Path $Directory 'release.json'))) '校验清单或发布信息摘要无效'
    $metadata = (Read-ApspText (Join-Path $Directory 'release.json') 32768) | ConvertFrom-Json
    Assert-Fields $metadata @('schema', 'source_repo', 'tag', 'protocol_min', 'protocol_max', 'artifacts')
    Assert-Apsp ((Test-Integer $metadata.schema) -and $metadata.schema -eq 1 -and $metadata.source_repo -ceq 'theLucius7/apsp' -and $metadata.tag -ceq ('agent-v' + $ExpectedVersion)) '发布身份与所选版本不匹配'
    Assert-Apsp ((Test-Integer $metadata.protocol_min) -and (Test-Integer $metadata.protocol_max) -and $metadata.protocol_min -ge 1 -and $metadata.protocol_min -le $metadata.protocol_max -and $metadata.protocol_max -le 65535) '发布协议范围无效'
    Assert-Apsp ($metadata.artifacts -is [Array] -and $metadata.artifacts.Count -ge 1 -and $metadata.artifacts.Count -le 30) '发布制品列表无效'
    $expected = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    [void]$expected.Add('release.json'); [void]$expected.Add('install.sh')
    foreach ($entry in $metadata.artifacts) {
        $fields = @('name', 'version', 'arch', 'format', 'binary_name', 'archive_size', 'binary_sha256', 'binary_size', 'asset_name')
        if ($entry.PSObject.Properties.Name -contains 'auxiliary_files') { $fields += 'auxiliary_files' }
        Assert-Fields $entry $fields
        Assert-Apsp ($entry.name -cin @('agent', 'sing-box', 'nodequality', 'tcpquality') -and $entry.version -cmatch '^[0-9A-Za-z][0-9A-Za-z.+_-]{0,127}$' -and $entry.arch -cin $script:Targets -and $entry.format -cin @('raw', 'tar.gz')) '制品身份无效'
        Assert-Apsp ($entry.binary_name -cmatch '^[0-9A-Za-z][0-9A-Za-z.+_-]{0,127}$' -and (Test-Integer $entry.archive_size) -and (Test-Integer $entry.binary_size) -and $entry.archive_size -gt 0 -and $entry.archive_size -le 268435456 -and $entry.binary_size -gt 0 -and $entry.binary_size -le 268435456 -and $entry.binary_sha256 -cmatch '^[0-9a-f]{64}$') '制品大小或摘要无效'
        $path = $entry.name + '/' + $entry.version + '/' + $entry.arch
        Assert-Apsp ($expected.Add($path)) '重复的制品身份'
        if ($entry.arch -cin @('amd64', 'arm64')) {
            $asset = if ($entry.format -ceq 'raw') { $entry.name + '-' + $entry.version + '-linux-musl-' + $entry.arch } else { $entry.name + '-' + $entry.version + '-linux-' + $entry.arch + '.tar.gz' }
        } else {
            $asset = $entry.name + '-' + $entry.version + '-' + $entry.arch
            if ($entry.format -ceq 'tar.gz') { $asset += '.tar.gz' }
        }
        Assert-Apsp ($entry.asset_name -ceq $asset) '制品下载名称与签名身份不匹配'
        if ($entry.format -ceq 'raw') {
            Assert-Apsp ($entry.archive_size -eq $entry.binary_size -and $rows.ContainsKey($path) -and $rows[$path] -ceq $entry.binary_sha256) '裸二进制签名大小或摘要无效'
        }
        if ($entry.name -ceq 'agent') {
            $binaryName = if ($entry.arch.StartsWith('windows-')) { 'apsp-agent.exe' } else { 'apsp-agent' }
            Assert-Apsp ($entry.version -ceq $ExpectedVersion -and $entry.format -ceq 'raw' -and $entry.binary_name -ceq $binaryName) 'Agent 身份不匹配'
        }
        if ($entry.PSObject.Properties.Name -contains 'auxiliary_files') {
            $auxiliary = $entry.auxiliary_files
            Assert-Apsp ($auxiliary -is [pscustomobject] -and @($auxiliary.PSObject.Properties).Count -le 7) '附属文件列表无效'
            foreach ($property in $auxiliary.PSObject.Properties) {
                Assert-Apsp ($entry.format -ceq 'tar.gz' -and $property.Name -cmatch '^[0-9A-Za-z._-]{1,128}$' -and -not $property.Name.StartsWith('-') -and $property.Name -cnotin @('.', '..', $entry.binary_name, 'release.json', 'SHA256SUMS', 'SHA256SUMS.minisig', '.artifact.json')) '附属文件名无效'
                Assert-Fields $property.Value @('sha256', 'size')
                Assert-Apsp ((Test-Integer $property.Value.size) -and $property.Value.size -gt 0 -and $property.Value.size -le 268435456 -and $property.Value.sha256 -cmatch '^[0-9a-f]{64}$') '附属文件大小或摘要无效'
            }
        }
    }
    Assert-Apsp ($expected.SetEquals($rows.Keys) -and $rows['install.sh'] -ceq (Get-ApspHash (Join-Path $Directory 'install.sh'))) '签名清单包含多余文件或安装器摘要不匹配'
    return $metadata
}
function Get-ReleaseCandidates([string]$Directory, [string]$Origin, [string]$EnrollmentToken, [string]$Actual, [string]$RequestedVersion) {
    $url = $Origin.TrimEnd('/') + '/api/bootstrap/versions?target=' + [Uri]::EscapeDataString($Actual) + '&token=' + [Uri]::EscapeDataString($EnrollmentToken)
    if ($RequestedVersion -ne 'latest') { $url += '&agent_version=' + [Uri]::EscapeDataString($RequestedVersion) }
    $path = Join-Path $Directory 'versions.json'
    Receive-ApspFile $url $path 131072
    $index = (Read-ApspText $path 131072) | ConvertFrom-Json
    Assert-Apsp ($index.PSObject.Properties.Name -contains 'versions' -and $index.versions -is [Array] -and $index.versions.Count -le 256) '版本目录格式无效'
    $versions = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $candidates = @(foreach ($entry in $index.versions) {
        Assert-Apsp ($entry.version -is [string] -and $entry.version -cmatch $script:VersionPattern -and $entry.tag -ceq ('agent-v' + $entry.version)) '版本目录身份无效'
        if (($RequestedVersion -eq 'latest' -and $entry.version -cmatch '^[0-9]+\.[0-9]+\.[0-9]+$') -or $entry.version -ceq $RequestedVersion) {
            if ($versions.Add($entry.version)) { $entry.version }
        }
    })
    Assert-Apsp ($candidates.Count -gt 0) ('面板未导入本机 ' + $Actual + ' 可用的签名 Agent 版本，请先导入 Release')
    return @($candidates | Sort-Object -Descending { [Version](($_ -split '[-+]')[0]) })
}
function Invoke-CheckedAgent([string]$Agent, [string[]]$Arguments) {
    # Require this invocation to start and complete successfully, not an older exit code.
    $global:LASTEXITCODE = $null
    & $Agent @Arguments
    $succeeded = $?
    $exitCode = $global:LASTEXITCODE
    Assert-Apsp ($succeeded -and $null -ne $exitCode -and $exitCode -eq 0) 'Agent 验证、接入或服务安装失败，请检查输出'
}
function Assert-ProtectedPath([string]$Path) {
    $item = Get-Item -LiteralPath $Path -Force
    Assert-Apsp (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0) '安装路径禁止重解析点'
    $acl = Get-Acl -LiteralPath $Path
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    $trusted = @('S-1-5-18', 'S-1-5-32-544', [Security.Principal.WindowsIdentity]::GetCurrent().User.Value, 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
    Assert-Apsp ($owner -in $trusted) '安装路径所有者不可信'
    $write = [Security.AccessControl.FileSystemRights]::Write -bor [Security.AccessControl.FileSystemRights]::Delete -bor [Security.AccessControl.FileSystemRights]::ChangePermissions -bor [Security.AccessControl.FileSystemRights]::TakeOwnership -bor [Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles
    foreach ($rule in $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) {
        if ($rule.AccessControlType -eq [Security.AccessControl.AccessControlType]::Allow -and ($rule.PropagationFlags -band [Security.AccessControl.PropagationFlags]::InheritOnly) -eq 0 -and ($rule.FileSystemRights -band $write) -ne 0) {
            Assert-Apsp ($rule.IdentityReference.Value -in $trusted) '安装路径允许非管理员写入'
        }
    }
}
function Assert-ProtectedTree([string]$Path) {
    $current = [IO.Path]::GetFullPath($Path)
    while ($current) {
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            Assert-Apsp (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0) '安装路径祖先禁止重解析点'
            # Drive roots and ProgramData permit creating new child directories;
            # the existing Apsp boundary must deny replacing children or writes.
            if ($current -ieq $Path -or $current -ieq (Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'Apsp')) {
                Assert-ProtectedPath $current
            } else {
                $acl = Get-Acl -LiteralPath $current
                $dangerous = [Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles -bor [Security.AccessControl.FileSystemRights]::Delete -bor [Security.AccessControl.FileSystemRights]::ChangePermissions -bor [Security.AccessControl.FileSystemRights]::TakeOwnership
                $trusted = @('S-1-5-18', 'S-1-5-32-544', [Security.Principal.WindowsIdentity]::GetCurrent().User.Value, 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
                Assert-Apsp ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -in $trusted) '安装路径祖先所有者不可信'
                foreach ($rule in $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) {
                    if ($rule.AccessControlType -eq [Security.AccessControl.AccessControlType]::Allow -and ($rule.PropagationFlags -band [Security.AccessControl.PropagationFlags]::InheritOnly) -eq 0 -and ($rule.FileSystemRights -band $dangerous) -ne 0) {
                        Assert-Apsp ($rule.IdentityReference.Value -in $trusted) '安装路径祖先允许非管理员替换或改权'
                    }
                }
            }
        }
        $parent = [IO.Directory]::GetParent($current)
        $current = if ($null -ne $parent) { $parent.FullName } else { $null }
    }
}
function Get-PreviousAgent([string]$Root, [string]$Configuration) {
    $core = Join-Path $Root 'core'
    if (Test-Path -LiteralPath $Configuration) {
        $text = Read-ApspText $Configuration 1048576
        # Config serialization uses either a literal or escaped single-line TOML string.
        $literal = [regex]::Match($text, "(?m)^agent_root\s*=\s*'([^'\r\n]+)'\s*$")
        $escaped = [regex]::Match($text, '(?m)^agent_root\s*=\s*("(?:[^"\\\r\n]|\\.)*")\s*$')
        if ($literal.Success) { $core = $literal.Groups[1].Value }
        elseif ($escaped.Success) { $core = $escaped.Groups[1].Value | ConvertFrom-Json }
        elseif ($text -match '(?m)^agent_root\s*=') { throw '无法解析已有 Agent 安装目录，请检查配置' }
    }
    $reference = Join-Path $core 'current'
    if (-not (Test-Path -LiteralPath $reference)) { return $null }
    Assert-ProtectedTree $reference
    $value = (Read-ApspText $reference 16384) | ConvertFrom-Json
    Assert-Fields $value @('apsp_directory_reference', 'target')
    Assert-Apsp ($value.apsp_directory_reference -is [bool] -and $value.apsp_directory_reference -and $value.target -is [string]) '已有 Agent 版本引用无效'
    $destination = $value.target
    if (-not [IO.Path]::IsPathRooted($destination)) { $destination = Join-Path $core $destination }
    $destination = [IO.Path]::GetFullPath($destination)
    Assert-Apsp ([IO.Directory]::GetParent($destination).FullName -ieq [IO.Path]::GetFullPath($core) -and [IO.Path]::GetFileName($destination) -cmatch $script:VersionPattern) '已有 Agent 版本引用逃离安装目录'
    Assert-ProtectedTree $destination
    $binary = Join-Path $destination 'apsp-agent.exe'
    Assert-ProtectedTree $binary
    return $binary
}
function Assert-PartialIdentity([string]$Identity, [string]$Origin) {
    if (-not (Test-Path -LiteralPath $Identity)) { return }
    Assert-ProtectedTree $Identity
    $files = @(Get-ChildItem -LiteralPath $Identity -Force)
    if ($files.Count -eq 0) { return }
    foreach ($file in $files) {
        Assert-Apsp (-not $file.PSIsContainer -and $file.Name -cin @('device.key', 'panel_origin', 'server_id')) '缺少配置的身份目录包含未知状态，请先修复安装'
        Assert-ProtectedTree $file.FullName
    }
    $key = Join-Path $Identity 'device.key'
    $recorded = Join-Path $Identity 'panel_origin'
    Assert-Apsp (Test-Path -LiteralPath $recorded) '部分接入身份缺少面板来源，请先修复安装'
    if (Test-Path -LiteralPath $key) { Assert-Apsp ((Get-Item -LiteralPath $key).Length -eq 32) '部分接入设备密钥格式无效' }
    $recordedOrigin = (Read-ApspText $recorded 4096).Trim()
    Assert-Origin $recordedOrigin
    $expected = ([Uri]$Origin).GetLeftPart([UriPartial]::Authority)
    Assert-Apsp ($recordedOrigin -ceq $expected) '已有设备身份属于其他面板，请先修复安装'
    $id = Join-Path $Identity 'server_id'
    if (Test-Path -LiteralPath $id) {
        Assert-Apsp (Test-Path -LiteralPath $key) '已有服务器身份缺少设备密钥'
        [long]$number = 0
        Assert-Apsp ([long]::TryParse((Read-ApspText $id 64).Trim(), [ref]$number) -and $number -gt 0) '已有服务器身份无效'
    }
}
function New-ProtectedDirectory([string]$Parent) {
    Assert-ProtectedTree $Parent
    Assert-ProtectedPath $Parent
    $path = Join-Path $Parent ('apsp-bootstrap-' + [Guid]::NewGuid().ToString('N'))
    $security = [Security.AccessControl.DirectorySecurity]::new()
    $security.SetAccessRuleProtection($true, $false)
    $administrators = [Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')
    $security.SetOwner($administrators)
    foreach ($sid in @('S-1-5-18', 'S-1-5-32-544')) {
        $rule = [Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new($sid), [Security.AccessControl.FileSystemRights]::FullControl, [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit', [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow)
        $security.AddAccessRule($rule)
    }
    # Create under a protected parent; set explicit ACL before putting executable bytes there.
    [void][IO.Directory]::CreateDirectory($path)
    Set-Acl -LiteralPath $path -AclObject $security
    Assert-ProtectedPath $path
    return $path
}
function Invoke-ApspBootstrap {
    Assert-Apsp ($env:OS -eq 'Windows_NT') '此入口用于 Windows；Linux、macOS 和 FreeBSD 请使用 Shell 入口'
    $administrator = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
    Assert-Apsp ($administrator.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) '请在管理员 PowerShell 中执行安装命令'
    Assert-Origin $Panel
    Assert-Mirror $Mirror $Panel
    $mirrorHost = if ($Mirror) { ([Uri]$Mirror).Host } else { '' }
    Assert-Apsp ($Version -eq 'latest' -or $Version -cmatch '^(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)$') 'Windows 服务安装仅支持稳定版本，请选择自动匹配或数字三段版本'
    $actual = Get-HostTarget
    Assert-Apsp ($Target -eq 'auto' -or $Target -ceq $actual) ('所选平台与本机 ' + $actual + ' 不兼容')
    $directory = New-ProtectedDirectory ([Environment]::GetFolderPath('Windows'))
    try {
        $minisign = Get-Minisign $directory $actual
        $candidates = @(Get-ReleaseCandidates $directory $Panel $Token $actual $Version)
        $selected = $null
        foreach ($candidate in $candidates) {
            $bundle = Join-Path $directory $candidate
            [void][IO.Directory]::CreateDirectory($bundle)
            $base = 'https://github.com/theLucius7/apsp/releases/download/agent-v' + $candidate
            foreach ($file in @(@('SHA256SUMS', 8192), @('SHA256SUMS.minisig', 16384), @('release.json', 32768), @('install.sh', 262144))) {
                Receive-ApspFile (Get-ReleaseUrl ($base + '/' + $file[0]) $Mirror) (Join-Path $bundle $file[0]) $file[1] $true 0 $mirrorHost
            }
            Test-ReleaseSignature $bundle $minisign
            $metadata = Read-ReleaseManifest $bundle $candidate
            if ($metadata.protocol_min -gt 1 -or $metadata.protocol_max -lt 1) { continue }
            $matches = @($metadata.artifacts | Where-Object { $_.name -ceq 'agent' -and $_.version -ceq $candidate -and $_.arch -ceq $actual })
            if ($matches.Count -eq 1) { $selected = $matches[0]; break }
        }
        Assert-Apsp ($null -ne $selected) ('已签名发布缺少本机 ' + $actual + ' 兼容版本；请先发布并导入对应制品')
        $agent = Join-Path $bundle 'apsp-agent.exe'
        $url = Get-ReleaseUrl ($base + '/' + $selected.asset_name) $Mirror
        # GitHub and mirror requests never carry an enrollment token or device credentials.
        Receive-ApspFile $url $agent $selected.binary_size $true $selected.binary_size $mirrorHost
        Assert-Apsp ((Get-ApspHash $agent) -ceq $selected.binary_sha256) 'Agent 不符合已签摘要，拒绝执行'
        Invoke-CheckedAgent $agent @('verify-installed', '--binary', $agent, '--name', 'agent', '--format', 'raw')
        $programData = [Environment]::GetFolderPath('CommonApplicationData')
        $env:ProgramData = $programData
        $root = Join-Path $programData 'Apsp'
        $configuration = Join-Path $root 'agent.toml'
        $backup = $null
        $previousAgent = $null
        Assert-ProtectedTree $root
        if (Test-Path -LiteralPath $configuration) {
            Assert-ProtectedTree $configuration
            Invoke-CheckedAgent $agent @('--config', $configuration, 'verify-cache')
            $backup = [IO.File]::ReadAllBytes($configuration)
            $previousAgent = Get-PreviousAgent $root $configuration
            if ($null -ne $previousAgent) { Invoke-CheckedAgent $agent @('verify-installed', '--binary', $previousAgent, '--name', 'agent', '--format', 'raw') }
        } else {
            foreach ($path in @((Join-Path $root 'core'), (Join-Path $root 'plugins'), (Join-Path $root 'state'))) {
                Assert-Apsp (-not (Test-Path -LiteralPath $path)) '已有 Agent 状态但缺少配置，请先修复已有安装'
            }
            Assert-PartialIdentity (Join-Path $root 'identity') $Panel
        }
        try {
            Invoke-CheckedAgent $agent @('--config', $configuration, 'enroll', '--panel', $Panel, ('--token=' + $Token))
            Invoke-CheckedAgent $agent @('--config', $configuration, 'install-service')
        } catch {
            $failure = $_
            if ($null -ne $backup) {
                [IO.File]::WriteAllBytes($configuration, $backup)
                if ($null -ne $previousAgent -and (Test-Path -LiteralPath $previousAgent)) {
                    Invoke-CheckedAgent $agent @('verify-installed', '--binary', $previousAgent, '--name', 'agent', '--format', 'raw')
                    Invoke-CheckedAgent $previousAgent @('--config', $configuration, 'install-service')
                }
            }
            throw $failure
        }
        Write-Host ('Agent ' + $selected.version + ' 已接入并安装为 Windows 计划任务')
    } finally { Remove-Item -LiteralPath $directory -Recurse -Force }
}
if ($MyInvocation.InvocationName -ne '.') { Invoke-ApspBootstrap }

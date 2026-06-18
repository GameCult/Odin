param(
  [string] $RavenHost = "raven",
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $LocalMuninnExe = "",
  [int] $ConnectTimeoutSeconds = 10,
  [string] $SshUser = "",
  [string] $IdentityFile = "",
  [switch] $SkipBuild,
  [switch] $SkipRestart,
  [switch] $PreflightOnly
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot

if ([string]::IsNullOrWhiteSpace($LocalMuninnExe)) {
  $LocalMuninnExe = Join-Path $repoRoot "target\release\muninn.exe"
}

if (-not $SkipBuild) {
  Push-Location $repoRoot
  try {
    cargo build -p muninn-daemon --release
    if ($LASTEXITCODE -ne 0) {
      throw "cargo build -p muninn-daemon --release failed with exit code $LASTEXITCODE"
    }
  } finally {
    Pop-Location
  }
}

if (-not (Test-Path -LiteralPath $LocalMuninnExe)) {
  throw "Local Muninn executable was not found at '$LocalMuninnExe'."
}

if ($ConnectTimeoutSeconds -lt 1) {
  throw "ConnectTimeoutSeconds must be at least 1."
}

if (-not $SkipRestart) {
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "verify-muninn-media-profile.ps1") `
    -RestartScript (Join-Path $PSScriptRoot "restart-muninn.ps1")
  if ($LASTEXITCODE -ne 0) {
    throw "verify-muninn-media-profile.ps1 failed with exit code $LASTEXITCODE"
  }
}

function Set-AsciiFile {
  param(
    [Parameter(Mandatory = $true)] [string] $Path,
    [Parameter(Mandatory = $true)] [string] $Content
  )

  [System.IO.File]::WriteAllText($Path, ($Content -replace "`r?`n", "`r`n"), [System.Text.Encoding]::ASCII)
}

function ConvertTo-PowerShellStringLiteral {
  param([Parameter(Mandatory = $true)] [string] $Value)
  return "'" + $Value.Replace("'", "''") + "'"
}

function Get-SshCommonArgs {
  param([int] $TimeoutSeconds = $ConnectTimeoutSeconds)

  $args = @(
    "-o", "BatchMode=yes",
    "-o", "ConnectTimeout=$TimeoutSeconds",
    "-o", "ConnectionAttempts=1"
  )
  if (-not [string]::IsNullOrWhiteSpace($IdentityFile)) {
    $args += @("-i", $IdentityFile)
  }
  return $args
}

function Get-SshTarget {
  param([Parameter(Mandatory = $true)] [string] $Target)

  if ([string]::IsNullOrWhiteSpace($SshUser)) {
    return $Target
  }
  return "${SshUser}@${Target}"
}

function Get-SshEffectiveConfig {
  param([Parameter(Mandatory = $true)] [string] $Target)

  $previousErrorActionPreference = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    $commonArgs = Get-SshCommonArgs
    $sshTarget = Get-SshTarget -Target $Target
    $output = & ssh.exe -G @commonArgs $sshTarget 2>&1
  } finally {
    $ErrorActionPreference = $previousErrorActionPreference
  }

  $config = @{
    hostname = $Target
    user = $null
    identityfile = @()
  }
  if ($LASTEXITCODE -ne 0) {
    $config.error = ($output -join " ")
    return $config
  }

  foreach ($line in $output) {
    if ($line -match "^hostname\s+(.+)$") {
      $config.hostname = $Matches[1]
    } elseif ($line -match "^user\s+(.+)$") {
      $config.user = $Matches[1]
    } elseif ($line -match "^identityfile\s+(.+)$") {
      $config.identityfile += $Matches[1]
    }
  }

  return $config
}

function Test-TcpPort {
  param(
    [Parameter(Mandatory = $true)] [string] $HostName,
    [Parameter(Mandatory = $true)] [int] $Port,
    [Parameter(Mandatory = $true)] [int] $TimeoutSeconds
  )

  $client = [System.Net.Sockets.TcpClient]::new()
  try {
    $async = $client.BeginConnect($HostName, $Port, $null, $null)
    if (-not $async.AsyncWaitHandle.WaitOne([TimeSpan]::FromSeconds($TimeoutSeconds))) {
      return "timeout"
    }
    $client.EndConnect($async)
    return "reachable"
  } catch {
    return $_.Exception.Message
  } finally {
    $client.Close()
  }
}

function Test-IcmpHost {
  param(
    [Parameter(Mandatory = $true)] [string] $HostName,
    [Parameter(Mandatory = $true)] [int] $TimeoutSeconds
  )

  $ping = [System.Net.NetworkInformation.Ping]::new()
  try {
    $reply = $ping.Send($HostName, [Math]::Max(1, $TimeoutSeconds) * 1000)
    if ($null -ne $reply -and $reply.Status -eq [System.Net.NetworkInformation.IPStatus]::Success) {
      return "reachable"
    }
    if ($null -ne $reply) {
      return $reply.Status.ToString()
    }
    return "unreachable"
  } catch {
    return $_.Exception.Message
  } finally {
    $ping.Dispose()
  }
}

function Get-RavenSshDiagnostic {
  param(
    [Parameter(Mandatory = $true)] [string] $Target,
    [Parameter(Mandatory = $true)] [int] $TimeoutSeconds
  )

  $config = Get-SshEffectiveConfig -Target $Target
  $hostName = [string] $config.hostname
  $icmp = Test-IcmpHost -HostName $hostName -TimeoutSeconds $TimeoutSeconds
  $tcp22 = Test-TcpPort -HostName $hostName -Port 22 -TimeoutSeconds $TimeoutSeconds
  $identity = if ($config.identityfile.Count -gt 0) {
    ($config.identityfile -join ",")
  } else {
    "default"
  }
  $user = if ([string]::IsNullOrWhiteSpace($config.user)) { "default" } else { $config.user }
  $configError = if ($config.ContainsKey("error")) { "; ssh_config_error=$($config.error)" } else { "" }

  return "resolved_host=$hostName user=$user identityfile=$identity icmp=$icmp tcp22=$tcp22$configError"
}

function Invoke-RavenSshPreflight {
  param(
    [Parameter(Mandatory = $true)] [string] $Target,
    [Parameter(Mandatory = $true)] [int] $TimeoutSeconds
  )

  $sshArgs = @(
    (Get-SshTarget -Target $Target),
    "cmd /c echo raven-ssh-ready"
  )
  $previousErrorActionPreference = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    $commonArgs = Get-SshCommonArgs -TimeoutSeconds $TimeoutSeconds
    $output = & ssh.exe @commonArgs @sshArgs 2>&1
  } finally {
    $ErrorActionPreference = $previousErrorActionPreference
  }
  if ($LASTEXITCODE -ne 0) {
    $diagnostic = Get-RavenSshDiagnostic -Target $Target -TimeoutSeconds $TimeoutSeconds
    throw "Raven SSH preflight failed for '$Target' within ${TimeoutSeconds}s: $($output -join ' '); $diagnostic"
  }
}

Invoke-RavenSshPreflight -Target $RavenHost -TimeoutSeconds $ConnectTimeoutSeconds

if ($PreflightOnly) {
  Write-Host "Raven SSH preflight succeeded for '$RavenHost'."
  exit 0
}

$deployId = [guid]::NewGuid().ToString("N")
$localTempRoot = Join-Path $env:TEMP "odin-raven-muninn-binary-$deployId"
$localRemoteScript = Join-Path $localTempRoot "deploy-raven-muninn-binary.ps1"
$localSftpBatch = Join-Path $localTempRoot "deploy-raven-muninn-binary.sftp"
$remoteExeSftpPath = "C:/Windows/Temp/muninn-$deployId.exe"
$remoteScriptSftpPath = "C:/Windows/Temp/deploy-raven-muninn-binary-$deployId.ps1"
$remoteExePath = "C:\Windows\Temp\muninn-$deployId.exe"
$remoteScriptPath = "C:\Windows\Temp\deploy-raven-muninn-binary-$deployId.ps1"
$remoteBackupPath = "$MuninnExe.bak-$deployId"

try {
  New-Item -ItemType Directory -Force -Path $localTempRoot | Out-Null

  $remoteScript = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"
`$target = $(ConvertTo-PowerShellStringLiteral $MuninnExe)
`$incoming = $(ConvertTo-PowerShellStringLiteral $remoteExePath)
`$backup = $(ConvertTo-PowerShellStringLiteral $remoteBackupPath)

if (-not (Test-Path -LiteralPath `$incoming)) {
  throw "Uploaded Muninn executable was not found at `$incoming"
}

New-Item -ItemType Directory -Force -Path (Split-Path -Parent `$target) | Out-Null

Get-CimInstance Win32_Process |
  Where-Object { `$_.Name -ieq "muninn.exe" } |
  ForEach-Object {
    & taskkill.exe /PID `$_.ProcessId /T /F | Out-Null
  }

if (Test-Path -LiteralPath `$target) {
  Copy-Item -LiteralPath `$target -Destination `$backup -Force
}

Move-Item -LiteralPath `$incoming -Destination `$target -Force
`$item = Get-Item -LiteralPath `$target
Write-Output ("muninn.exe deployed length={0} path={1}" -f `$item.Length, `$item.FullName)
"@

  Set-AsciiFile -Path $localRemoteScript -Content $remoteScript
  Set-AsciiFile -Path $localSftpBatch -Content (@(
    'put "{0}" "{1}"' -f $LocalMuninnExe, $remoteExeSftpPath
    'put "{0}" "{1}"' -f $localRemoteScript, $remoteScriptSftpPath
  ) -join "`r`n")

  $sftpArgs = @(
    "-b", $localSftpBatch,
    (Get-SshTarget -Target $RavenHost)
  )
  $commonArgs = Get-SshCommonArgs
  & sftp.exe @commonArgs @sftpArgs
  if ($LASTEXITCODE -ne 0) {
    throw "sftp upload to Raven failed with exit code $LASTEXITCODE"
  }

  $remoteRunner = @"
`$ErrorActionPreference = "Stop"
try {
  & "$remoteScriptPath"
  `$code = `$LASTEXITCODE
} finally {
  Remove-Item -LiteralPath "$remoteScriptPath" -Force -ErrorAction SilentlyContinue
}
exit `$code
"@
  $encodedRunner = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remoteRunner))
  $commonArgs = Get-SshCommonArgs
  $sshTarget = Get-SshTarget -Target $RavenHost
  & ssh.exe @commonArgs $sshTarget "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand $encodedRunner"
  if ($LASTEXITCODE -ne 0) {
    throw "remote Muninn binary deploy failed with exit code $LASTEXITCODE"
  }
} finally {
  Remove-Item -LiteralPath $localTempRoot -Recurse -Force -ErrorAction SilentlyContinue
}

if (-not $SkipRestart) {
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "restart-muninn.ps1") `
    -RavenHost $RavenHost `
    -MuninnExe $MuninnExe `
    -ConnectTimeoutSeconds $ConnectTimeoutSeconds `
    -SshUser $SshUser `
    -IdentityFile $IdentityFile
  if ($LASTEXITCODE -ne 0) {
    throw "restart-muninn.ps1 failed with exit code $LASTEXITCODE"
  }
}

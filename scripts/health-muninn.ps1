param(
  [string] $RavenHost = "raven-local",
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\muninn.telemetry.cc",
  [string] $ActivateStorePath = "C:\Meta\Odin\state\muninn.activate.cc",
  [string] $LogRoot = "C:\Meta\Odin\logs\muninn",
  [int] $MaxStoreAgeSeconds = 180,
  [string] $IdunnRudpHealth = $env:IDUNN_RUDP_HEALTH,
  [int] $ConnectTimeoutSeconds = 10,
  [string] $SshUser = "madman's lullaby",
  [string] $IdentityFile = ""
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($IdunnRudpHealth)) {
  throw "Idunn RUDP health endpoint must be supplied by -IdunnRudpHealth or IDUNN_RUDP_HEALTH; no Starfire LAN default is allowed."
}

function Set-AsciiFile {
  param(
    [Parameter(Mandatory = $true)] [string] $Path,
    [Parameter(Mandatory = $true)] [string] $Content
  )

  [System.IO.File]::WriteAllText($Path, ($Content -replace "`r?`n", "`r`n"), [System.Text.Encoding]::ASCII)
}

function Get-SshCommonArgs {
  $args = @(
    "-o", "BatchMode=yes",
    "-o", "ConnectTimeout=$ConnectTimeoutSeconds",
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

function Invoke-RavenUploadedPowerShell {
  param(
    [Parameter(Mandatory = $true)] [string] $RavenHost,
    [Parameter(Mandatory = $true)] [string] $RemoteScriptContent,
    [Parameter(Mandatory = $true)] [string] $TempPrefix
  )

  $uploadId = [guid]::NewGuid().ToString("N")
  $localTempRoot = Join-Path $env:TEMP "$TempPrefix-$uploadId"
  $localRemoteScript = Join-Path $localTempRoot "$TempPrefix-$uploadId.ps1"
  $localSftpBatch = Join-Path $localTempRoot "$TempPrefix-$uploadId.sftp"
  $remoteSftpPath = "C:/Windows/Temp/$TempPrefix-$uploadId.ps1"
  $remotePsPath = "C:\Windows\Temp\$TempPrefix-$uploadId.ps1"

  try {
    New-Item -ItemType Directory -Force -Path $localTempRoot | Out-Null
    Set-AsciiFile -Path $localRemoteScript -Content $RemoteScriptContent
    Set-AsciiFile -Path $localSftpBatch -Content ('put "{0}" "{1}"' -f $localRemoteScript, $remoteSftpPath)

    $commonArgs = Get-SshCommonArgs
    $sshTarget = Get-SshTarget -Target $RavenHost
    & sftp.exe @commonArgs -b $localSftpBatch $sshTarget
    if ($LASTEXITCODE -ne 0) {
      exit $LASTEXITCODE
    }

    $remoteRunner = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"
try {
  & "$remotePsPath"
  exit 0
} finally {
  Remove-Item -LiteralPath "$remotePsPath" -Force -ErrorAction SilentlyContinue
}
"@
    $encodedRunner = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remoteRunner))
    $commonArgs = Get-SshCommonArgs
    $sshTarget = Get-SshTarget -Target $RavenHost
    & ssh.exe @commonArgs $sshTarget "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -OutputFormat Text -EncodedCommand $encodedRunner"
  } finally {
    Remove-Item -LiteralPath $localTempRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}

$remoteScript = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"
if (-not (Test-Path -LiteralPath "$MuninnExe")) {
  throw "Muninn executable not found at $MuninnExe"
}
`$muninnDir = Split-Path -Parent "$MuninnExe"
`$taskExpectations = @(
  @{ Name = "GameCult-Muninn"; Launcher = "powershell"; Vbs = Join-Path `$muninnDir "start-muninn-serve-hidden.vbs"; Ps = Join-Path `$muninnDir "start-muninn-serve.ps1" }
)
foreach (`$expectation in `$taskExpectations) {
  `$task = Get-ScheduledTask -TaskName `$expectation.Name -ErrorAction Stop
  `$action = @(`$task.Actions)[0]
  if (`$expectation.Launcher -eq "powershell") {
    if (`$action.Execute -notmatch 'powershell\.exe$') {
      throw "`$(`$expectation.Name) action executes `$(`$action.Execute), expected powershell.exe"
    }
    if (`$action.Arguments -notlike "*`$(`$expectation.Ps)*") {
      throw "`$(`$expectation.Name) action arguments `$(`$action.Arguments) do not reference `$(`$expectation.Ps)"
    }
    if (`$action.Arguments -notlike "*-File*") {
      throw "`$(`$expectation.Name) action arguments `$(`$action.Arguments) do not execute a PowerShell launcher"
    }
  } else {
    if (`$action.Execute -notmatch '(^|\\)wscript\.exe$') {
      throw "`$(`$expectation.Name) action executes `$(`$action.Execute), expected wscript.exe"
    }
    if (`$action.Arguments -notlike "*`$(`$expectation.Vbs)*") {
      throw "`$(`$expectation.Name) action arguments `$(`$action.Arguments) do not reference `$(`$expectation.Vbs)"
    }
    if (`$action.Arguments -notlike "*//B*" -or `$action.Arguments -notlike "*//Nologo*") {
      throw "`$(`$expectation.Name) action arguments `$(`$action.Arguments) do not force hidden WScript execution"
    }
  }
  if (-not (Test-Path -LiteralPath `$expectation.Vbs)) {
    throw "Missing hidden launcher for `$(`$expectation.Name) at `$(`$expectation.Vbs)"
  }
  if (-not (Test-Path -LiteralPath `$expectation.Ps)) {
    throw "Missing PowerShell launcher for `$(`$expectation.Name) at `$(`$expectation.Ps)"
  }
  `$psContent = Get-Content -LiteralPath `$expectation.Ps -Raw
  if (`$psContent -like "*'' | ConvertFrom-Json*") {
    throw "`$(`$expectation.Ps) has an empty encoded-arguments payload"
  }
  if (`$psContent -notmatch 'Start-Process') {
    throw "`$(`$expectation.Ps) does not launch a hidden process"
  }
  `$vbs = Get-Content -LiteralPath `$expectation.Vbs -Raw
  if (`$vbs -match 'cmdPath\s*=') {
    throw "`$(`$expectation.Vbs) still routes through a cmdPath trampoline"
  }
  if (`$vbs -notmatch '\.ps1') {
    throw "`$(`$expectation.Vbs) does not reference a PowerShell launcher"
  }
}
if (Get-ScheduledTask -TaskName "GameCult-Muninn-Activate" -ErrorAction SilentlyContinue) {
  throw "Obsolete GameCult-Muninn-Activate task is still present on Raven"
}
if (Get-ScheduledTask -TaskName "GameCult-Muninn-VideoProof" -ErrorAction SilentlyContinue) {
  throw "Obsolete GameCult-Muninn-VideoProof task is still present on Raven"
}
`$servePidPath = Join-Path "$LogRoot" "muninn-serve.pid"
`$servePid = `$null
if (Test-Path -LiteralPath `$servePidPath) {
  `$servePidText = (Get-Content -LiteralPath `$servePidPath -Raw).Trim()
  if (`$servePidText -match '^\d+$') {
    `$servePid = [int] `$servePidText
  }
}
`$process = `$null
if (`$null -ne `$servePid) {
  `$process = Get-CimInstance Win32_Process -Filter ("ProcessId = {0}" -f `$servePid) -ErrorAction SilentlyContinue |
    Where-Object { `$_.Name -ieq "muninn.exe" -and `$_.CommandLine -like "*serve*" } |
    Select-Object -First 1
}
if (`$null -eq `$process) {
  `$process = Get-CimInstance Win32_Process |
    Where-Object { `$_.Name -ieq "muninn.exe" -and `$_.CommandLine -like "*serve*" } |
    Select-Object -First 1
}
if (`$null -eq `$process) {
  throw "Muninn serve process is not running"
}
foreach (`$pattern in @(
  "--host raven",
  "--activate-store $ActivateStorePath",
  "--idunn-rudp-health $IdunnRudpHealth",
  "--idunn-daemon muninn",
  "--idunn-health-contract muninn.cultnet-rudp-remote-telemetry-health"
)) {
  if (`$process.CommandLine -notlike "*`$pattern*") {
    throw "Muninn serve process is missing expected command-line segment `${pattern}: `$(`$process.CommandLine)"
  }
}
`$conflictingWriter = Get-CimInstance Win32_Process |
  Where-Object {
    `$_.Name -like "muninn*.exe" -and
    `$_.ProcessId -ne `$process.ProcessId -and
    `$_.ParentProcessId -ne `$process.ProcessId -and
    `$_.CommandLine -like "*activate*" -and
    `$_.CommandLine -like "*$StorePath*"
  } |
  Select-Object -First 1
if (`$null -ne `$conflictingWriter) {
  throw "Conflicting Muninn activate writer still targets ${StorePath}: `$(`$conflictingWriter.CommandLine)"
}
if (-not (Test-Path -LiteralPath "$StorePath")) {
  throw "Muninn telemetry store is missing at $StorePath"
}
`$storeAgeSeconds = (([DateTime]::UtcNow) - (Get-Item -LiteralPath "$StorePath").LastWriteTimeUtc).TotalSeconds
if (`$storeAgeSeconds -gt $MaxStoreAgeSeconds) {
  throw "Muninn telemetry store is stale (`$([math]::Round(`$storeAgeSeconds))s old)"
}
if (-not (Test-Path -LiteralPath `$servePidPath)) {
  throw "Muninn serve PID file is missing at `$servePidPath"
}
if (`$null -ne `$servePid -and `$process.ProcessId -ne `$servePid) {
  throw "Muninn serve PID file references `$servePid but live Muninn serve process is `$(`$process.ProcessId)"
}
"@

Invoke-RavenUploadedPowerShell -RavenHost $RavenHost -RemoteScriptContent $remoteScript -TempPrefix "odin-raven-muninn-health"
exit $LASTEXITCODE

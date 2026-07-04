param(
  [string] $RavenHost = "raven",
  [string] $SleipnirExe = "C:\Meta\Odin\Sleipnir\sleipnir.exe",
  [string] $StorePath = "C:\Meta\Odin\state\raven.sleipnir.cc",
  [string] $LogRoot = "C:\Meta\Odin\logs\Sleipnir",
  [string] $HostId = "raven",
  [string] $OdinCultMeshUri = $(if ($env:ODIN_CULTMESH_URI) { $env:ODIN_CULTMESH_URI } else { "cultmesh://odin/rendezvous/provider-catalog" }),
  [string] $RudpBind = "0.0.0.0:17890",
  [string] $CommandRoute = $(if ($env:SLEIPNIR_COMMAND_ROUTE) { $env:SLEIPNIR_COMMAND_ROUTE } else { "cultmesh://odin/command/sleipnir/input_mapping" }),
  [string] $IdunnRudpHealth = $env:IDUNN_RUDP_HEALTH,
  [string] $IdunnDaemon = "raven-sleipnir",
  [string] $IdunnHealthContract = "sleipnir.cultnet-rudp-input-mirror-health",
  [string] $IntervalMs = "8",
  [string] $SshUser = "madman's lullaby",
  [string] $IdentityFile = "C:\Users\Meta\.ssh\id_ed25519_192_168_1_84",
  [int] $ConnectTimeoutSeconds = 10
)

$ErrorActionPreference = "Stop"

if ($env:IDUNN_ACTUATOR -ne "1" -or $env:IDUNN_COMMAND_AUTHORITY -ne "idunn-daemon") {
  throw "restart-raven-sleipnir.ps1 is an Idunn actuator body. Redeploy by poking Idunn; direct service restart is not an owned path."
}
if ([string]::IsNullOrWhiteSpace($OdinCultMeshUri) -or -not $OdinCultMeshUri.StartsWith("cultmesh://", [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "Sleipnir Odin publication requires -OdinCultMeshUri or ODIN_CULTMESH_URI and must be a cultmesh:// URI."
}
if ([string]::IsNullOrWhiteSpace($CommandRoute) -or -not $CommandRoute.StartsWith("cultmesh://")) {
  throw "Sleipnir command route must be a cultmesh:// URI supplied by -CommandRoute or SLEIPNIR_COMMAND_ROUTE."
}
if ([string]::IsNullOrWhiteSpace($IdunnRudpHealth)) {
  throw "Sleipnir Idunn health endpoint requires -IdunnRudpHealth or IDUNN_RUDP_HEALTH; no Starfire LAN default is allowed."
}

function ConvertTo-PowerShellStringLiteral {
  param([Parameter(Mandatory = $true)] [string] $Value)
  return "'" + $Value.Replace("'", "''") + "'"
}

function Get-SshTarget {
  if ([string]::IsNullOrWhiteSpace($SshUser)) {
    return $RavenHost
  }
  return "${SshUser}@${RavenHost}"
}

$sshArgs = @(
  "-o", "BatchMode=yes",
  "-o", "ConnectTimeout=$ConnectTimeoutSeconds",
  "-o", "ConnectionAttempts=1"
)
if (-not [string]::IsNullOrWhiteSpace($IdentityFile)) {
  $sshArgs += @("-i", $IdentityFile)
}

$argPairs = @(
  @("--store", $StorePath),
  @("--host", $HostId),
  @("--odin-cultmesh-uri", $OdinCultMeshUri),
  @("--rudp-bind", $RudpBind),
  @("--command-route", $CommandRoute),
  @("--idunn-rudp-health", $IdunnRudpHealth),
  @("--idunn-daemon", $IdunnDaemon),
  @("--idunn-health-contract", $IdunnHealthContract),
  @("--interval-ms", $IntervalMs)
)
$flatArgumentLiterals = @()
foreach ($pair in $argPairs) {
  $flatArgumentLiterals += ConvertTo-PowerShellStringLiteral $pair[0]
  $flatArgumentLiterals += ConvertTo-PowerShellStringLiteral $pair[1]
}
if ($env:SLEIPNIR_TRACE -eq "1") {
  $flatArgumentLiterals += ConvertTo-PowerShellStringLiteral "--trace"
}
$argumentLiteral = "@(" + ($flatArgumentLiterals -join ", ") + ")"

$remote = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"
`$PSDefaultParameterValues["*:ProgressAction"] = "SilentlyContinue"
if (-not (Test-Path -LiteralPath $(ConvertTo-PowerShellStringLiteral $SleipnirExe))) {
  throw "Sleipnir executable not found at $SleipnirExe"
}
Get-CimInstance Win32_Process |
  Where-Object { `$_.Name -ieq "sleipnir.exe" -and `$_.CommandLine -like "*--store*$StorePath*" } |
  ForEach-Object { taskkill.exe /PID `$_.ProcessId /T /F | Out-Null }
New-Item -ItemType Directory -Force -Path $(ConvertTo-PowerShellStringLiteral $LogRoot) | Out-Null
`$storeParent = Split-Path -Parent $(ConvertTo-PowerShellStringLiteral $StorePath)
if (-not [string]::IsNullOrWhiteSpace(`$storeParent)) {
  New-Item -ItemType Directory -Force -Path `$storeParent | Out-Null
}
`$taskName = "Odin-Sleipnir-Raven"
`$arguments = $argumentLiteral
`$outLog = Join-Path $(ConvertTo-PowerShellStringLiteral $LogRoot) "sleipnir.out.log"
`$errLog = Join-Path $(ConvertTo-PowerShellStringLiteral $LogRoot) "sleipnir.err.log"
function Quote-RemotePowerShellString([string] `$Value) {
  return "'" + `$Value.Replace("'", "''") + "'"
}
`$child = @'
`$ErrorActionPreference = "Continue"
`$ProgressPreference = "SilentlyContinue"
& __SLEIPNIR_EXE__ __SLEIPNIR_ARGS__ 1>> __OUT_LOG__ 2>> __ERR_LOG__
'@
`$child = `$child.Replace("__SLEIPNIR_EXE__", $(ConvertTo-PowerShellStringLiteral $SleipnirExe))
`$child = `$child.Replace("__SLEIPNIR_ARGS__", (`$arguments | ForEach-Object { "'" + `$_.Replace("'", "''") + "'" }) -join " ")
`$child = `$child.Replace("__OUT_LOG__", (Quote-RemotePowerShellString `$outLog))
`$child = `$child.Replace("__ERR_LOG__", (Quote-RemotePowerShellString `$errLog))
`$encodedChild = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes(`$child))
`$action = New-ScheduledTaskAction -Execute "`$env:WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe" -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -EncodedCommand `$encodedChild"
`$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(5)
`$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero)
Register-ScheduledTask -TaskName `$taskName -Action `$action -Trigger `$trigger -Settings `$settings -Force | Out-Null
Remove-Item -LiteralPath `$outLog -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath `$errLog -Force -ErrorAction SilentlyContinue
Start-ScheduledTask -TaskName `$taskName
Start-Sleep -Seconds 5
`$running = Get-CimInstance Win32_Process | Where-Object { `$_.Name -ieq "sleipnir.exe" -and `$_.CommandLine -like "*--store*$StorePath*" } | Select-Object -First 1
if (`$null -eq `$running) {
  `$errorText = if (Test-Path -LiteralPath `$errLog) { Get-Content -LiteralPath `$errLog -Raw } else { "" }
  throw "Sleipnir did not stay running. `$errorText"
}
"Sleipnir Raven restarted under scheduled task `$taskName pid=`$(`$running.ProcessId)"
"@

$target = Get-SshTarget
$remoteScript = "$SleipnirExe.idunn-restart.ps1"
$remoteScriptSftp = $remoteScript.Replace("\", "/")
$localScript = New-TemporaryFile
$sftpBatch = New-TemporaryFile
try {
  Set-Content -LiteralPath $localScript.FullName -Value $remote -Encoding ASCII
  Set-Content -LiteralPath $sftpBatch.FullName -Value @("put `"$($localScript.FullName)`" `"$remoteScriptSftp`"") -Encoding ASCII
  & sftp.exe @sshArgs -b $sftpBatch.FullName $target
  if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
  }

  $invoke = @"
`$ErrorActionPreference = "Stop"
try {
  & $(ConvertTo-PowerShellStringLiteral $remoteScript) 3>`$null
  `$code = if (`$null -eq `$LASTEXITCODE) { 0 } else { `$LASTEXITCODE }
} finally {
  Remove-Item -LiteralPath $(ConvertTo-PowerShellStringLiteral $remoteScript) -Force -ErrorAction SilentlyContinue
}
exit `$code
"@
  $encodedInvoke = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($invoke))
  & ssh.exe @sshArgs $target "powershell.exe -NoProfile -ExecutionPolicy Bypass -EncodedCommand $encodedInvoke"
  if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
  }
} finally {
  Remove-Item -LiteralPath $localScript.FullName -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $sftpBatch.FullName -Force -ErrorAction SilentlyContinue
}

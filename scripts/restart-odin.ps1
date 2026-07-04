param(
  [string] $StateDir = "E:\Projects\Odin\scratch\odin",
  [string] $CultNetRudpBind = "0.0.0.0:17871",
  [string] $IdunnRudpHealth = $(if ($env:ODIN_IDUNN_RUDP_HEALTH) { $env:ODIN_IDUNN_RUDP_HEALTH } else { $env:IDUNN_RUDP_HEALTH }),
  [string] $IdunnDaemon = "odin",
  [string] $IdunnHealthContract = "odin.cultnet-rudp-provider-health"
)

$ErrorActionPreference = "Stop"

if ($env:IDUNN_ACTUATOR -ne "1" -or $env:IDUNN_COMMAND_AUTHORITY -ne "idunn-daemon") {
  throw "restart-odin.ps1 is an Idunn actuator body. Redeploy by poking Idunn; direct service restart is not an owned path."
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$stopScript = Join-Path $PSScriptRoot "stop-odin.ps1"
$startScript = Join-Path $PSScriptRoot "start-odin.ps1"
$powerShellExe = Join-Path $env:WINDIR "System32\WindowsPowerShell\v1.0\powershell.exe"

if (-not (Test-Path -LiteralPath $powerShellExe)) {
  throw "PowerShell executable is missing at $powerShellExe"
}

$startArgs = @(
  "-NoProfile",
  "-ExecutionPolicy", "Bypass",
  "-File", $startScript,
  "-StateDir", $StateDir,
  "-CultNetRudpBind", $CultNetRudpBind,
  "-IdunnDaemon", $IdunnDaemon,
  "-IdunnHealthContract", $IdunnHealthContract
)

if (-not [string]::IsNullOrWhiteSpace($IdunnRudpHealth)) {
  $startArgs += @("-IdunnRudpHealth", $IdunnRudpHealth)
}

& $powerShellExe -NoProfile -ExecutionPolicy Bypass -File $stopScript -StateDir $StateDir
if ($LASTEXITCODE -ne 0) {
  throw "stop-odin.ps1 failed with exit code $LASTEXITCODE"
}

& $powerShellExe @startArgs
if ($LASTEXITCODE -ne 0) {
  throw "start-odin.ps1 failed with exit code $LASTEXITCODE"
}

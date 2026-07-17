param(
  [string] $StateDir = "E:\Projects\Odin\scratch\odin",
  [string] $CultNetRudpBind = "0.0.0.0:17871",
  [string] $IdunnRudpHealth = $(if ($env:ODIN_IDUNN_RUDP_HEALTH) { $env:ODIN_IDUNN_RUDP_HEALTH } else { $env:IDUNN_RUDP_HEALTH }),
  [string] $IdunnDaemon = "odin",
  [string] $IdunnHealthContract = "odin.cultnet-rudp-provider-health",
  [string] $CultLibPackages = $(if ($env:CULTLIB_PACKAGES) { $env:CULTLIB_PACKAGES } else { "E:\Projects\CultLib-dev-runtime\packages" }),
  [switch] $Foreground
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$scriptPath = Join-Path $repoRoot "src\odin-coordinator.cjs"
$pidPath = Join-Path $StateDir "odin.pid"
$outLog = Join-Path $StateDir "odin.out.log"
$errLog = Join-Path $StateDir "odin.err.log"
$nodeExe = (Get-Command node.exe -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Source)
if (-not $nodeExe) {
  $nodeExe = "C:\Program Files\nodejs\node.exe"
}
if (-not (Test-Path -LiteralPath $nodeExe)) {
  throw "Node.js executable is missing; checked PATH and $nodeExe"
}

if ([string]::IsNullOrWhiteSpace($IdunnRudpHealth)) {
  throw "Odin Idunn health publication requires -IdunnRudpHealth, ODIN_IDUNN_RUDP_HEALTH, or IDUNN_RUDP_HEALTH; no localhost default is assumed."
}

New-Item -ItemType Directory -Force -Path $StateDir | Out-Null

if (Test-Path $pidPath) {
  $oldPid = (Get-Content -Raw $pidPath).Trim()
  if ($oldPid -match "^\d+$") {
    $old = Get-Process -Id ([int] $oldPid) -ErrorAction SilentlyContinue
    if ($old) {
      throw "Odin already appears to be running as PID $oldPid."
    }
  }
  Remove-Item -LiteralPath $pidPath -Force
}

if (-not (Test-Path -LiteralPath $CultLibPackages)) {
  throw "CultLib package root is missing: $CultLibPackages"
}
$env:CULTLIB_PACKAGES = $CultLibPackages
$env:NODE_PATH = $CultLibPackages
$args = @(
  $scriptPath,
  "--stateDir", $StateDir,
  "--cultnet-rudp-bind", $CultNetRudpBind,
  "--idunn-rudp-health", $IdunnRudpHealth,
  "--idunn-daemon", $IdunnDaemon,
  "--idunn-health-contract", $IdunnHealthContract
)

if ($Foreground) {
  & $nodeExe @args
  exit $LASTEXITCODE
}

$proc = Start-Process -FilePath $nodeExe -ArgumentList $args -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput $outLog -RedirectStandardError $errLog
$proc.Id | Set-Content -Encoding ASCII -LiteralPath $pidPath
Start-Sleep -Seconds 1
if ($proc.HasExited) {
  $detail = ""
  if (Test-Path $outLog) { $detail += Get-Content -Raw $outLog }
  if (Test-Path $errLog) { $detail += Get-Content -Raw $errLog }
  throw "Odin exited immediately with code $($proc.ExitCode).`n$detail"
}

Write-Host "Odin started as PID $($proc.Id)."
Write-Host "CultMesh/RUDP document catalog: $CultNetRudpBind"

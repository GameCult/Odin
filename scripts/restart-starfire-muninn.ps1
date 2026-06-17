param(
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\starfire.muninn.telemetry.cc",
  [string] $LogRoot = "C:\Meta\Odin\logs\starfire-muninn",
  [string] $QuestSerial = "1WMHHB68PG1515",
  [string] $MoveBluetoothHost = "5C:93:A2:9C:A8:A8",
  [string[]] $MoveState = @(),
  [switch] $EnableUsbMoveState,
  [string] $IdunnRudpHealth = "127.0.0.1:17870",
  [string] $IdunnDaemon = "starfire-muninn",
  [string] $IdunnHealthContract = "muninn.cultnet-rudp-local-telemetry-and-quest-access"
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $MuninnExe)) {
  throw "Muninn executable not found at $MuninnExe"
}

Get-CimInstance Win32_Process |
  Where-Object {
    $_.Name -ieq "muninn.exe" -and
    $_.CommandLine -like "*serve*" -and
    $_.CommandLine -like "*--host*starfire*"
  } |
  ForEach-Object {
    & taskkill.exe /PID $_.ProcessId /T /F | Out-Null
  }

New-Item -ItemType Directory -Force -Path $LogRoot | Out-Null
$storeParent = Split-Path -Parent $StorePath
if (-not [string]::IsNullOrWhiteSpace($storeParent)) {
  New-Item -ItemType Directory -Force -Path $storeParent | Out-Null
}

if (-not [string]::IsNullOrWhiteSpace($MoveBluetoothHost)) {
  & $MuninnExe claim-move-host --move-host $MoveBluetoothHost
}

$arguments = @(
  "serve",
  "--store", $StorePath,
  "--log-root", $LogRoot,
  "--host", "starfire",
  "--interval-seconds", "15",
  "--quest-adb",
  "--quest-serial", $QuestSerial,
  "--idunn-rudp-health", $IdunnRudpHealth,
  "--idunn-daemon", $IdunnDaemon,
  "--idunn-health-contract", $IdunnHealthContract
)
if (-not [string]::IsNullOrWhiteSpace($MoveBluetoothHost)) {
  $arguments += @("--move-host", $MoveBluetoothHost)
}
foreach ($source in $MoveState) {
  if (-not [string]::IsNullOrWhiteSpace($source)) {
    $arguments += @("--move-state", $source)
  }
}
if ($EnableUsbMoveState) {
  $arguments += @("--move-state", "move-starfire-usb=windows-psmove")
}

$serveOutLog = Join-Path $LogRoot "muninn-serve.out.log"
$serveErrLog = Join-Path $LogRoot "muninn-serve.err.log"
$pidPath = Join-Path $LogRoot "muninn-serve.pid"
$lockPath = "$StorePath.lock"

function Start-MuninnServeProcess {
  param([string[]] $ArgumentList)

  $process = Start-Process `
    -FilePath $MuninnExe `
    -ArgumentList $ArgumentList `
    -WindowStyle Hidden `
    -PassThru `
    -RedirectStandardOutput $serveOutLog `
    -RedirectStandardError $serveErrLog
  $process.Id | Set-Content -Encoding ASCII -LiteralPath $pidPath
  return $process
}

function Get-StarfireMuninnServeProcess {
  return Get-CimInstance Win32_Process |
    Where-Object {
      $_.Name -ieq "muninn.exe" -and
      $_.CommandLine -like "*serve*" -and
      $_.CommandLine -like "*--host*starfire*"
    } |
    Select-Object -First 1
}

function Reset-CorruptMuninnStore {
  $timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
  if (Test-Path -LiteralPath $StorePath) {
    Move-Item -LiteralPath $StorePath -Destination "$StorePath.corrupt-$timestamp" -Force
  }
  Remove-Item -LiteralPath $lockPath -Force -ErrorAction SilentlyContinue
  $storeLeaf = Split-Path -Leaf $StorePath
  Get-ChildItem -LiteralPath $storeParent -ErrorAction SilentlyContinue | Where-Object {
    $_.Name -like "$storeLeaf.*.tmp"
  } | ForEach-Object {
    Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue
  }
}

Remove-Item -LiteralPath $lockPath -Force -ErrorAction SilentlyContinue

$null = Start-MuninnServeProcess -ArgumentList $arguments

Start-Sleep -Seconds 2
$processCheck = Get-StarfireMuninnServeProcess
if ($null -eq $processCheck) {
  $serveError = if (Test-Path -LiteralPath $serveErrLog) {
    Get-Content -LiteralPath $serveErrLog -Raw
  } else {
    ""
  }
  if ($serveError -like "*failed to decode MessagePack*") {
    Reset-CorruptMuninnStore
    $null = Start-MuninnServeProcess -ArgumentList $arguments
    Start-Sleep -Seconds 2
    $processCheck = Get-StarfireMuninnServeProcess
  }
}
if ($null -eq $processCheck) {
  throw "Starfire Muninn serve process is not running after restart"
}
foreach ($pattern in @(
  "--idunn-rudp-health",
  $IdunnRudpHealth,
  "--idunn-daemon",
  $IdunnDaemon,
  "--idunn-health-contract",
  $IdunnHealthContract
)) {
  if ($processCheck.CommandLine -notlike "*$pattern*") {
    throw "Starfire Muninn serve command line is missing ${pattern}: $($processCheck.CommandLine)"
  }
}
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "health-starfire-muninn.ps1") `
  -MuninnExe $MuninnExe `
  -StorePath $StorePath

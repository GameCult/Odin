param(
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\starfire.muninn.telemetry.cc",
  [string] $LogRoot = "C:\Meta\Odin\logs\starfire-muninn",
  [string[]] $MoveState = @(),
  [int] $MaxStoreAgeSeconds = 180,
  [string] $IdunnRudpHealth = "127.0.0.1:17870"
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $MuninnExe)) {
  throw "Muninn executable not found at $MuninnExe"
}

$process = Get-CimInstance Win32_Process |
  Where-Object {
    $_.Name -ieq "muninn.exe" -and
    $_.CommandLine -like "*serve*" -and
    $_.CommandLine -like "*--host*starfire*" -and
    $_.CommandLine -like "*--quest-adb*"
  } |
  Select-Object -First 1
if ($null -eq $process) {
  throw "Starfire Muninn serve process is not running with Quest access enabled"
}
foreach ($pattern in @(
  "--idunn-rudp-health $IdunnRudpHealth",
  "--idunn-daemon starfire-muninn",
  "--idunn-health-contract muninn.cultnet-rudp-local-telemetry-and-quest-access"
)) {
  if ($process.CommandLine -notlike "*$pattern*") {
    throw "Starfire Muninn serve process is missing expected command-line segment ${pattern}: $($process.CommandLine)"
  }
}
if (-not (Test-Path -LiteralPath $StorePath)) {
  throw "Starfire Muninn telemetry store is missing at $StorePath"
}
$storeAgeSeconds = (([DateTime]::UtcNow) - (Get-Item -LiteralPath $StorePath).LastWriteTimeUtc).TotalSeconds
if ($storeAgeSeconds -gt $MaxStoreAgeSeconds) {
  throw "Starfire Muninn telemetry store is stale ($([math]::Round($storeAgeSeconds))s old)"
}
$pidPath = Join-Path $LogRoot "muninn-serve.pid"
if (-not (Test-Path -LiteralPath $pidPath)) {
  throw "Starfire Muninn serve PID file is missing at $pidPath"
}

param(
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\starfire.muninn.telemetry.cc",
  [string] $LogRoot = "C:\Meta\Odin\logs\starfire-muninn",
  [string] $QuestSerial = "1WMHHB68PG1515",
  [string] $MoveBluetoothHost = "5C:93:A2:9C:A8:A8",
  [string[]] $MoveState = @(),
  [switch] $EnableUsbMoveState,
  [string] $IdunnRudpHealth = $env:IDUNN_RUDP_HEALTH,
  [string] $IdunnDaemon = "starfire-muninn",
  [string] $IdunnHealthContract = "muninn.cultnet-rudp-local-telemetry-and-quest-access",
  [string] $OdinCultMeshUri = $(if ($env:ODIN_CULTMESH_URI) { $env:ODIN_CULTMESH_URI } else { "cultmesh://odin/rendezvous/provider-catalog" }),
  [string] $OdinCultMeshRudpEndpoint = $(if ($env:CULTMESH_URI_ODIN_RUDP) { $env:CULTMESH_URI_ODIN_RUDP } else { "127.0.0.1:17871" }),
  [string] $HidControllerRudpBind = "0.0.0.0:17888",
  [string] $HidControllerRudpAdvertise = $(if ($env:MUNINN_HID_CONTROLLER_RUDP_ADVERTISE) { $env:MUNINN_HID_CONTROLLER_RUDP_ADVERTISE } else { "10.77.0.2:17888" })
)

$ErrorActionPreference = "Stop"

if ($env:IDUNN_ACTUATOR -ne "1" -or $env:IDUNN_COMMAND_AUTHORITY -ne "idunn-daemon") {
  throw "restart-starfire-muninn.ps1 is an Idunn actuator body. Redeploy by poking Idunn; direct service restart is not an owned path."
}

if (-not (Test-Path -LiteralPath $MuninnExe)) {
  throw "Muninn executable not found at $MuninnExe"
}
if ([string]::IsNullOrWhiteSpace($IdunnRudpHealth)) {
  throw "Idunn RUDP health endpoint must be supplied by -IdunnRudpHealth or IDUNN_RUDP_HEALTH; no Starfire LAN default is allowed."
}
if (-not [string]::IsNullOrWhiteSpace($HidControllerRudpBind) -and [string]::IsNullOrWhiteSpace($HidControllerRudpAdvertise)) {
  throw "HID controller RUDP advertise endpoint must be supplied by -HidControllerRudpAdvertise or MUNINN_HID_CONTROLLER_RUDP_ADVERTISE when HID RUDP bind is enabled; no Starfire LAN default is allowed."
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
if (-not [string]::IsNullOrWhiteSpace($OdinCultMeshUri)) {
  $arguments += @("--odin-cultmesh-uri", $OdinCultMeshUri)
}
if (-not [string]::IsNullOrWhiteSpace($HidControllerRudpBind)) {
  $arguments += @("--hid-controller-rudp-bind", $HidControllerRudpBind)
}
if (-not [string]::IsNullOrWhiteSpace($HidControllerRudpAdvertise)) {
  $arguments += @("--hid-controller-rudp-advertise", $HidControllerRudpAdvertise)
}
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

  $previousOdinEndpoint = $env:CULTMESH_URI_ODIN_RUDP
  try {
    $env:CULTMESH_URI_ODIN_RUDP = $OdinCultMeshRudpEndpoint
    $process = Start-Process `
      -FilePath $MuninnExe `
      -ArgumentList $ArgumentList `
      -WindowStyle Hidden `
      -PassThru `
      -RedirectStandardOutput $serveOutLog `
      -RedirectStandardError $serveErrLog
  } finally {
    $env:CULTMESH_URI_ODIN_RUDP = $previousOdinEndpoint
  }
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

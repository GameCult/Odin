param(
  [string] $StateDir = "E:\Projects\Odin\scratch\odin",
  [int] $MaxSilenceSeconds = 120,
  [string] $CultNetRudpBind = "0.0.0.0:17871"
)

$ErrorActionPreference = "Stop"

$pidPath = Join-Path $StateDir "odin.pid"
$outLog = Join-Path $StateDir "odin.out.log"
$errLog = Join-Path $StateDir "odin.err.log"
$cachePath = Join-Path $StateDir "odin.ccmp"

if (-not (Test-Path -LiteralPath $pidPath)) {
  throw "Odin PID file is missing at $pidPath"
}

$pidText = (Get-Content -LiteralPath $pidPath -Raw).Trim()
if ($pidText -notmatch '^\d+$') {
  throw "Odin PID file is invalid at ${pidPath}: '$pidText'"
}

$process = Get-CimInstance Win32_Process -Filter "ProcessId = $pidText" -ErrorAction SilentlyContinue
if ($null -eq $process) {
  throw "Odin process $pidText is not running"
}

foreach ($pattern in @(
  'node.exe',
  'src\odin-coordinator.cjs',
  "--stateDir $StateDir",
  "--cultnet-rudp-bind $CultNetRudpBind",
  '--idunn-daemon odin',
  '--idunn-health-contract odin.cultnet-rudp-provider-health'
)) {
  if ($process.CommandLine -notlike "*$pattern*") {
    throw "Odin process command line is missing expected segment ${pattern}: $($process.CommandLine)"
  }
}

foreach ($logPath in @($outLog, $errLog)) {
  if (-not (Test-Path -LiteralPath $logPath)) {
    throw "Odin log is missing at $logPath"
  }
}

$newestLogWriteUtc = @($outLog, $errLog) |
  ForEach-Object { (Get-Item -LiteralPath $_).LastWriteTimeUtc } |
  Sort-Object -Descending |
  Select-Object -First 1
$logAgeSeconds = ([DateTime]::UtcNow - $newestLogWriteUtc).TotalSeconds
if ($logAgeSeconds -gt $MaxSilenceSeconds) {
  throw "Odin logs have been silent for $([math]::Round($logAgeSeconds))s, exceeding $MaxSilenceSeconds seconds"
}

if (-not (Test-Path -LiteralPath $cachePath)) {
  throw "Odin CultMesh store is missing at $cachePath"
}

$rudpPort = [int](($CultNetRudpBind -split ':')[-1])
$udpListener = Get-NetUDPEndpoint -LocalPort $rudpPort -ErrorAction SilentlyContinue |
  Where-Object { $_.OwningProcess -eq [int]$pidText } |
  Select-Object -First 1
if ($null -eq $udpListener) {
  throw "Odin is not listening on UDP port $rudpPort as process $pidText"
}

Write-Host "Odin healthy: pid=$pidText cultmesh_rudp_udp=$rudpPort store=$cachePath log_age=$([math]::Round($logAgeSeconds))s"

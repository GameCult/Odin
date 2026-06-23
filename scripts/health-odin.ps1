param(
  [int] $Port = 8797,
  [string] $StateDir = "E:\Projects\Odin\scratch\odin",
  [int] $MaxSilenceSeconds = 120,
  [string] $CultNetRudpBind = "0.0.0.0:17871"
)

$ErrorActionPreference = "Stop"

$pidPath = Join-Path $StateDir "odin.pid"
$outLog = Join-Path $StateDir "odin.out.log"
$errLog = Join-Path $StateDir "odin.err.log"

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
  "--port $Port",
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

$healthUri = "http://127.0.0.1:$Port/health"
try {
  $health = Invoke-RestMethod -Uri $healthUri -TimeoutSec 10
} catch {
  throw "Odin health endpoint is unavailable at ${healthUri}: $($_.Exception.Message)"
}

if (-not $health.ok) {
  throw "Odin health endpoint reports ok=false"
}

$tcpListener = Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue |
  Where-Object { $_.OwningProcess -eq [int]$pidText } |
  Select-Object -First 1
if ($null -eq $tcpListener) {
  throw "Odin is not listening on TCP port $Port as process $pidText"
}

$rudpPort = [int](($CultNetRudpBind -split ':')[-1])
$udpListener = Get-NetUDPEndpoint -LocalPort $rudpPort -ErrorAction SilentlyContinue |
  Where-Object { $_.OwningProcess -eq [int]$pidText } |
  Select-Object -First 1
if ($null -eq $udpListener) {
  throw "Odin is not listening on UDP port $rudpPort as process $pidText"
}

Write-Host "Odin healthy: pid=$pidText tcp=$Port udp=$rudpPort log_age=$([math]::Round($logAgeSeconds))s"

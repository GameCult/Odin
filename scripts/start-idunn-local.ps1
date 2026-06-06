param(
  [int] $IntervalSeconds = 30,
  [string] $StateDir = "E:\Projects\Odin\scratch\idunn"
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$idunnExe = Join-Path $repoRoot "target\debug\idunn.exe"
$logDir = Join-Path $StateDir "logs"
$pidDir = Join-Path $StateDir "pids"

New-Item -ItemType Directory -Force -Path $StateDir, $logDir, $pidDir | Out-Null

if (-not (Test-Path -LiteralPath $idunnExe)) {
  Push-Location -LiteralPath $repoRoot
  try {
    & cargo build -p idunn-daemon
    if ($LASTEXITCODE -ne 0) {
      throw "cargo build -p idunn-daemon failed with exit code $LASTEXITCODE"
    }
  } finally {
    Pop-Location
  }
}

$watchdogs = @(
  [pscustomobject]@{
    Id = "odin"
    Name = "Odin all-seer"
    Verse = "starfire.local"
    Health = "curl.exe -fsS http://127.0.0.1:8797/health >NUL"
    Restart = "$repoRoot\scripts\restart-odin.cmd"
  },
  [pscustomobject]@{
    Id = "mimir-eve-dashboard"
    Name = "Mimir Eve dashboard"
    Verse = "starfire.local"
    Health = "curl.exe -fsS http://127.0.0.1:8795/health >NUL"
    Restart = $null
  },
  [pscustomobject]@{
    Id = "nightwing-gjallar"
    Name = "Nightwing Gjallar framebuffer compositor"
    Verse = "nightwing.local"
    Health = "ssh -o BatchMode=yes -o ConnectTimeout=5 nightwing `"systemctl is-active --quiet gjallar.service`""
    Restart = "$repoRoot\scripts\restart-nightwing-gjallar.cmd"
  },
  [pscustomobject]@{
    Id = "nightwing-eve-dashboard"
    Name = "Nightwing Eve dashboard broker"
    Verse = "nightwing.local"
    Health = "ssh -o BatchMode=yes -o ConnectTimeout=5 nightwing `"systemctl is-active --quiet nightwing-eve-dashboard.service`""
    Restart = "$repoRoot\scripts\restart-nightwing-eve-dashboard.cmd"
  },
  [pscustomobject]@{
    Id = "nightwing-eve-browser-reference"
    Name = "Nightwing Eve browser reference"
    Verse = "nightwing.local"
    Health = "ssh -o BatchMode=yes -o ConnectTimeout=5 nightwing `"systemctl is-active --quiet nightwing-eve-browser-reference.service`""
    Restart = "$repoRoot\scripts\restart-nightwing-eve-browser-reference.cmd"
  }
)

function Test-LivePid {
  param([string] $Path)
  if (-not (Test-Path -LiteralPath $Path)) {
    return $false
  }
  $pidText = (Get-Content -LiteralPath $Path -Raw).Trim()
  if ($pidText -notmatch "^\d+$") {
    Remove-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    return $false
  }
  $process = Get-Process -Id ([int] $pidText) -ErrorAction SilentlyContinue
  if ($null -eq $process) {
    Remove-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    return $false
  }
  return $true
}

function Start-Watchdog {
  param($Watchdog)

  $pidPath = Join-Path $pidDir "$($Watchdog.Id).pid"
  if (Test-LivePid -Path $pidPath) {
    Write-Host "Idunn watchdog already running for $($Watchdog.Id)."
    return
  }

  $storePath = Join-Path $StateDir "$($Watchdog.Id).keepalive.cc"

  $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
  $startInfo.FileName = $idunnExe
  $startInfo.WorkingDirectory = $repoRoot
  $startInfo.UseShellExecute = $false
  $startInfo.CreateNoWindow = $true
  $startInfo.RedirectStandardOutput = $false
  $startInfo.RedirectStandardError = $false
  $arguments = @(
      "--daemon", $Watchdog.Id,
      "--name", $Watchdog.Name,
      "--verse", $Watchdog.Verse,
      "--store", $storePath,
      "--health-command", $Watchdog.Health,
      "--interval-seconds", "$IntervalSeconds"
    )
  if (-not [string]::IsNullOrWhiteSpace($Watchdog.Restart)) {
    $arguments += @("--restart-command", $Watchdog.Restart, "--execute")
  }
  $startInfo.Arguments = Join-WindowsArguments -Arguments $arguments

  $process = [System.Diagnostics.Process]::new()
  $process.StartInfo = $startInfo
  [void] $process.Start()
  $process.Id | Set-Content -Encoding ASCII -LiteralPath $pidPath

  Write-Host "Started Idunn watchdog $($Watchdog.Id) as PID $($process.Id)."
  Write-Host "  store: $storePath"
}

function ConvertTo-WindowsArgument {
  param([string] $Value)
  if ($Value -notmatch '[\s"]') {
    return $Value
  }
  return '"' + ($Value -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"'
}

function Join-WindowsArguments {
  param([object[]] $Arguments)
  return (($Arguments | ForEach-Object { ConvertTo-WindowsArgument -Value ([string] $_) }) -join " ")
}

foreach ($watchdog in $watchdogs) {
  Start-Watchdog -Watchdog $watchdog
}

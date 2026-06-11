param(
  [int] $StaleAfterSeconds = 120,
  [string] $StateDir = "E:\Projects\Odin\scratch\idunn"
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$idunnExe = Join-Path $repoRoot "target\debug\idunn.exe"
$operatorAlarmCommand = Join-Path $repoRoot "scripts\notify-idunn-operator-alarm.cmd"
$pidPath = Join-Path $StateDir "idunn.pid"
$storePath = Join-Path $StateDir "idunn.keepalive.cc"
$outLog = Join-Path $StateDir "idunn.out.log"
$errLog = Join-Path $StateDir "idunn.err.log"

New-Item -ItemType Directory -Force -Path $StateDir | Out-Null

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

function Test-IdunnSupervisorHealthy {
  param(
    [string] $PidPath,
    [string] $StorePath,
    [int] $StaleAfterSeconds
  )

  if (-not (Test-Path -LiteralPath $PidPath)) {
    return $false
  }

  $pidText = (Get-Content -LiteralPath $PidPath -Raw).Trim()
  if ($pidText -notmatch "^\d+$") {
    Remove-Item -LiteralPath $PidPath -Force -ErrorAction SilentlyContinue
    return $false
  }

  $process = Get-Process -Id ([int] $pidText) -ErrorAction SilentlyContinue
  if ($null -eq $process) {
    Remove-Item -LiteralPath $PidPath -Force -ErrorAction SilentlyContinue
    return $false
  }

  if (Test-Path -LiteralPath $StorePath) {
    $storeAgeSeconds = ([DateTime]::UtcNow - (Get-Item -LiteralPath $StorePath).LastWriteTimeUtc).TotalSeconds
    if ($storeAgeSeconds -le $StaleAfterSeconds) {
      return $true
    }

    Write-Host "Idunn swarm store is stale ($([math]::Round($storeAgeSeconds))s old). Restarting supervisor."
  } else {
    $processAgeSeconds = ([DateTime]::UtcNow - $process.StartTime.ToUniversalTime()).TotalSeconds
    if ($processAgeSeconds -le $StaleAfterSeconds) {
      return $true
    }

    Write-Host "Idunn supervisor never wrote $StorePath within $StaleAfterSeconds seconds. Restarting supervisor."
  }

  Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $PidPath -Force -ErrorAction SilentlyContinue
  return $false
}

if (Test-IdunnSupervisorHealthy -PidPath $pidPath -StorePath $storePath -StaleAfterSeconds $StaleAfterSeconds) {
  Write-Host "Idunn swarm supervisor already running."
  return
}

$arguments = @(
  "--swarm-profile", "starfire-local",
  "--repo-root", $repoRoot,
  "--store", $storePath,
  "--command-timeout-seconds", "30",
  "--execute"
)

if (Test-Path -LiteralPath $operatorAlarmCommand) {
  $arguments += @("--operator-alarm-command", $operatorAlarmCommand)
}

$process = Start-Process -FilePath $idunnExe `
  -ArgumentList $arguments `
  -WorkingDirectory $repoRoot `
  -WindowStyle Hidden `
  -PassThru `
  -RedirectStandardOutput $outLog `
  -RedirectStandardError $errLog
$process.Id | Set-Content -Encoding ASCII -LiteralPath $pidPath

Start-Sleep -Seconds 2
if ($process.HasExited) {
  $detail = ""
  if (Test-Path -LiteralPath $outLog) { $detail += Get-Content -Raw -LiteralPath $outLog }
  if (Test-Path -LiteralPath $errLog) { $detail += Get-Content -Raw -LiteralPath $errLog }
  throw "Idunn swarm supervisor exited immediately with code $($process.ExitCode).`n$detail"
}

Write-Host "Started Idunn swarm supervisor as PID $($process.Id)."
Write-Host "  store: $storePath"

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

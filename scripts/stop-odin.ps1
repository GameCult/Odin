param(
  [string] $StateDir = "E:\Projects\Odin\scratch\odin"
)

$ErrorActionPreference = "Stop"

$pidPath = Join-Path $StateDir "odin.pid"
if (-not (Test-Path $pidPath)) {
  Write-Host "No Odin PID file found at $pidPath."
  exit 0
}

$odinPid = (Get-Content -Raw $pidPath).Trim()
if ($odinPid -match "^\d+$") {
  $process = Get-Process -Id ([int] $odinPid) -ErrorAction SilentlyContinue
  if ($process) {
    Stop-Process -Id $process.Id
    Write-Host "Stopped Odin PID $odinPid."
  }
}

Remove-Item -LiteralPath $pidPath -Force

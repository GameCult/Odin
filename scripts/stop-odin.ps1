param(
  [string] $StateDir = "E:\Projects\Odin\scratch\odin"
)

$ErrorActionPreference = "Stop"

$pidPath = Join-Path $StateDir "odin.pid"
if (-not (Test-Path $pidPath)) {
  Write-Host "No Odin PID file found at $pidPath."
  exit 0
}

$pid = (Get-Content -Raw $pidPath).Trim()
if ($pid -match "^\d+$") {
  $process = Get-Process -Id ([int] $pid) -ErrorAction SilentlyContinue
  if ($process) {
    Stop-Process -Id $process.Id
    Write-Host "Stopped Odin PID $pid."
  }
}

Remove-Item -LiteralPath $pidPath -Force

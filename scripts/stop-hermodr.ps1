param(
  [string] $StateDir = "E:\Projects\Odin\scratch\Hermodr"
)

$ErrorActionPreference = "Stop"

$pidPath = Join-Path $StateDir "hermodr.pid"
if (-not (Test-Path $pidPath)) {
  Write-Host "No Hermodr PID file found at $pidPath."
  exit 0
}

$HermodrPid = (Get-Content -Raw $pidPath).Trim()
if ($HermodrPid -match "^\d+$") {
  $process = Get-Process -Id ([int] $HermodrPid) -ErrorAction SilentlyContinue
  if ($process) {
    Stop-Process -Id $process.Id
    Write-Host "Stopped Hermodr PID $HermodrPid."
  }
}

Remove-Item -LiteralPath $pidPath -Force

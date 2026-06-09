$ErrorActionPreference = "Stop"

$stonksRoot = "E:\Projects\Stonks"
$stateDir = Join-Path $stonksRoot "scratch\stonks"
$pidPath = Join-Path $stateDir "stonks.pid"
$startScript = Join-Path $stonksRoot "scripts\start-stonks.ps1"

if (Test-Path -LiteralPath $pidPath) {
  $pidText = (Get-Content -LiteralPath $pidPath -Raw).Trim()
  if ($pidText -match "^\d+$") {
    $process = Get-Process -Id ([int] $pidText) -ErrorAction SilentlyContinue
    if ($null -ne $process) {
      Stop-Process -Id $process.Id -Force
      Start-Sleep -Milliseconds 500
    }
  }
  Remove-Item -LiteralPath $pidPath -Force -ErrorAction SilentlyContinue
}

& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $startScript
exit $LASTEXITCODE

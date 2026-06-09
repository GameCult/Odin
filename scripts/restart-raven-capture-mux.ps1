param(
  [string] $RavenHost = "raven"
)

$script = Join-Path $PSScriptRoot "restart-muninn.ps1"
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $script -RavenHost $RavenHost
exit $LASTEXITCODE

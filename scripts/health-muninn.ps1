param(
  [string] $RavenHost = "raven",
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\muninn.telemetry.cc",
  [string] $IdunnRudpHealth = "10.77.0.2:17870"
)

$ErrorActionPreference = "Stop"

$remoteScript = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"
if (-not (Test-Path -LiteralPath "$MuninnExe")) {
  throw "Muninn executable not found at $MuninnExe"
}
`$process = Get-CimInstance Win32_Process |
  Where-Object { `$_.Name -ieq "muninn.exe" -and `$_.CommandLine -like "*serve*" } |
  Select-Object -First 1
if (`$null -eq `$process) {
  throw "Muninn serve process is not running"
}
`$healthArgs = @(
  "--health",
  "--store", "$StorePath",
  "--idunn-rudp-health", "$IdunnRudpHealth",
  "--idunn-daemon", "muninn",
  "--idunn-health-contract", "muninn.cultnet-rudp-remote-telemetry-health"
)
& "$MuninnExe" @healthArgs
"@

$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remoteScript))
& ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $RavenHost "powershell.exe -NoProfile -NonInteractive -EncodedCommand $encoded"
exit $LASTEXITCODE

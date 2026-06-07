param(
  [string] $RavenHost = "10.77.0.4",
  [string] $MimirRepo = "E:\Projects\Mimir",
  [string] $TargetHost = "10.77.0.2",
  [int] $Port = 5200,
  [string] $ObsTargetHost = "10.77.0.2",
  [int] $ObsPort = 5204
)

$ErrorActionPreference = "Stop"

$remoteScript = @"
`$ErrorActionPreference = "Stop"
`$startScript = Join-Path "$MimirRepo" "scripts\start-raven-daemon.ps1"
if (-not (Test-Path -LiteralPath `$startScript)) {
  throw "Raven Mimir start script not found at `$startScript"
}
Get-CimInstance Win32_Process |
  Where-Object { `$_.Name -ieq "dotnet.exe" -and `$_.CommandLine -like "*Mimir.RavenDaemon*" } |
  ForEach-Object {
    & taskkill.exe /PID `$_.ProcessId /T /F | Out-Null
  }
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File `$startScript -TargetHost "$TargetHost" -Port $Port -ObsTargetHost "$ObsTargetHost" -ObsPort $ObsPort
"@

$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remoteScript))
& ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $RavenHost "powershell.exe -NoProfile -EncodedCommand $encoded"
exit $LASTEXITCODE

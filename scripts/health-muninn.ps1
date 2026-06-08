param(
  [string] $RavenHost = "raven",
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\muninn.telemetry.cc",
  [string] $RemoteObsCatalogPath = "C:\Meta\Odin\state\muninn-obs-streams.tsv",
  [string] $LocalObsCatalogPath = "C:\Meta\Odin\state\muninn-obs-streams.tsv"
)

$ErrorActionPreference = "Stop"

$remoteScript = @"
`$ErrorActionPreference = "Stop"
if (-not (Test-Path -LiteralPath "$MuninnExe")) {
  throw "Muninn executable not found at $MuninnExe"
}
`$process = Get-CimInstance Win32_Process |
  Where-Object { `$_.Name -ieq "muninn.exe" -and `$_.CommandLine -like "*serve*" } |
  Select-Object -First 1
if (`$null -eq `$process) {
  throw "Muninn serve process is not running"
}
& "$MuninnExe" --health --store "$StorePath"
"@

$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remoteScript))
& ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $RavenHost "powershell.exe -NoProfile -EncodedCommand $encoded"
$healthExit = $LASTEXITCODE
if ($healthExit -eq 0) {
  $localParent = Split-Path -Parent $LocalObsCatalogPath
  if (-not [string]::IsNullOrWhiteSpace($localParent)) {
    New-Item -ItemType Directory -Force -Path $localParent | Out-Null
  }
  & ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $RavenHost "cmd /c type $RemoteObsCatalogPath" |
    Set-Content -Encoding UTF8 -LiteralPath $LocalObsCatalogPath
}
exit $healthExit

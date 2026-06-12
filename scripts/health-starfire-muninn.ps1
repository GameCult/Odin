param(
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\starfire.muninn.telemetry.cc"
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $MuninnExe)) {
  throw "Muninn executable not found at $MuninnExe"
}

$process = Get-CimInstance Win32_Process |
  Where-Object {
    $_.Name -ieq "muninn.exe" -and
    $_.CommandLine -like "*serve*" -and
    $_.CommandLine -like "*--host*starfire*" -and
    $_.CommandLine -like "*--quest-adb*"
  } |
  Select-Object -First 1
if ($null -eq $process) {
  throw "Starfire Muninn serve process is not running with Quest access enabled"
}

& $MuninnExe --health --store $StorePath
if ($LASTEXITCODE -ne 0) {
  exit $LASTEXITCODE
}

& $MuninnExe quest-access-status --store $StorePath
exit $LASTEXITCODE

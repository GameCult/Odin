param(
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\starfire.muninn.telemetry.cc",
  [string] $LogRoot = "C:\Meta\Odin\logs\starfire-muninn",
  [string] $QuestSerial = "1WMHHB68PG1515",
  [string[]] $MoveState = @(),
  [switch] $EnableUsbMoveState
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $MuninnExe)) {
  throw "Muninn executable not found at $MuninnExe"
}

Get-CimInstance Win32_Process |
  Where-Object {
    $_.Name -ieq "muninn.exe" -and
    $_.CommandLine -like "*serve*" -and
    $_.CommandLine -like "*--host*starfire*"
  } |
  ForEach-Object {
    & taskkill.exe /PID $_.ProcessId /T /F | Out-Null
  }

New-Item -ItemType Directory -Force -Path $LogRoot | Out-Null
$storeParent = Split-Path -Parent $StorePath
if (-not [string]::IsNullOrWhiteSpace($storeParent)) {
  New-Item -ItemType Directory -Force -Path $storeParent | Out-Null
}

$arguments = @(
  "serve",
  "--store", $StorePath,
  "--log-root", $LogRoot,
  "--host", "starfire",
  "--interval-seconds", "15",
  "--quest-adb",
  "--quest-serial", $QuestSerial
)
foreach ($source in $MoveState) {
  if (-not [string]::IsNullOrWhiteSpace($source)) {
    $arguments += @("--move-state", $source)
  }
}
if ($EnableUsbMoveState) {
  $arguments += @("--move-state", "move-starfire-usb=windows-psmove")
}

$process = Start-Process `
  -FilePath $MuninnExe `
  -ArgumentList $arguments `
  -WindowStyle Hidden `
  -PassThru `
  -RedirectStandardOutput (Join-Path $LogRoot "muninn-serve.out.log") `
  -RedirectStandardError (Join-Path $LogRoot "muninn-serve.err.log")

$process.Id | Set-Content -Encoding ASCII -LiteralPath (Join-Path $LogRoot "muninn-serve.pid")

Start-Sleep -Seconds 2
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "health-starfire-muninn.ps1") `
  -MuninnExe $MuninnExe `
  -StorePath $StorePath

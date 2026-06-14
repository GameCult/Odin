param(
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\starfire.muninn.telemetry.cc",
  [string] $LogRoot = "C:\Meta\Odin\logs\starfire-muninn",
  [string] $QuestSerial = "1WMHHB68PG1515",
  [string] $MimirRepoPath = "E:\Projects\Mimir",
  [string] $UsbMoveLightRgb = "#35ff6c",
  [int] $UsbMoveLightRefreshMs = 200,
  [switch] $SkipUsbMoveLight
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

$process = Start-Process `
  -FilePath $MuninnExe `
  -ArgumentList $arguments `
  -WindowStyle Hidden `
  -PassThru `
  -RedirectStandardOutput (Join-Path $LogRoot "muninn-serve.out.log") `
  -RedirectStandardError (Join-Path $LogRoot "muninn-serve.err.log")

$process.Id | Set-Content -Encoding ASCII -LiteralPath (Join-Path $LogRoot "muninn-serve.pid")

if (-not $SkipUsbMoveLight) {
  $moveLightScript = Join-Path $MimirRepoPath "scripts\start-starfire-move-light.ps1"
  if (-not (Test-Path -LiteralPath $moveLightScript)) {
    throw "Starfire USB Move light script not found at $moveLightScript"
  }
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $moveLightScript `
    -Rgb $UsbMoveLightRgb `
    -RefreshMs $UsbMoveLightRefreshMs `
    -HoldSeconds 0
}

Start-Sleep -Seconds 2
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "health-starfire-muninn.ps1") `
  -MuninnExe $MuninnExe `
  -StorePath $StorePath

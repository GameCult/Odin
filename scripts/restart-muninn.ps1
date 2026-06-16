param(
  [string] $RavenHost = "raven",
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\muninn.telemetry.cc",
  [string] $LogRoot = "C:\Meta\Odin\logs\muninn"
)

$ErrorActionPreference = "Stop"

$remoteScript = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"
if (-not (Test-Path -LiteralPath "$MuninnExe")) {
  throw "Muninn executable not found at $MuninnExe"
}
Get-CimInstance Win32_Process |
  Where-Object { `$_.Name -ieq "muninn.exe" -and `$_.CommandLine -like "*serve*" } |
  ForEach-Object {
    & taskkill.exe /PID `$_.ProcessId /T /F | Out-Null
  }
New-Item -ItemType Directory -Force -Path "$LogRoot" | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path -Parent "$StorePath") | Out-Null
`$muninnDir = Split-Path -Parent "$MuninnExe"
`$launcher = Join-Path `$muninnDir "start-muninn-serve.cmd"
`$psLauncher = Join-Path `$muninnDir "start-muninn-serve.ps1"
`$vbsLauncher = Join-Path `$muninnDir "start-muninn-serve-hidden.vbs"
`$pidPath = Join-Path "$LogRoot" "muninn-serve.pid"
`$psLines = @(
  '`$ErrorActionPreference = "Stop"',
  '`$ProgressPreference = "SilentlyContinue"',
  '`$process = Start-Process -FilePath "$MuninnExe" -ArgumentList @("serve", "--store", "$StorePath", "--log-root", "$LogRoot", "--host", "raven", "--interval-seconds", "15") -WindowStyle Hidden -PassThru -RedirectStandardOutput "$LogRoot\muninn-serve.out.log" -RedirectStandardError "$LogRoot\muninn-serve.err.log"',
  '`$process.Id | Set-Content -Encoding ASCII -LiteralPath "$LogRoot\muninn-serve.pid"'
)
Set-Content -LiteralPath `$psLauncher -Value `$psLines -Encoding ASCII
`$vbsLines = @(
  'Set fso = CreateObject("Scripting.FileSystemObject")',
  'scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)',
  'psLauncher = fso.BuildPath(scriptDir, "start-muninn-serve.ps1")',
  'Set shell = CreateObject("WScript.Shell")',
  'shell.Run "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File """ & psLauncher & """", 0, False'
)
Set-Content -LiteralPath `$vbsLauncher -Value `$vbsLines -Encoding ASCII
`$lines = @(
  "@echo off",
  "cd /d ""`$muninnDir""",
  "wscript.exe //B //Nologo ""`$vbsLauncher"""
)
Set-Content -LiteralPath `$launcher -Value `$lines -Encoding ASCII
cmd /c "schtasks /Delete /TN GameCult-Muninn /F 2>NUL"
cmd /c schtasks /Create /TN GameCult-Muninn /SC ONCE /ST 23:59 /TR `$launcher /RL LIMITED /F
cmd /c schtasks /Run /TN GameCult-Muninn
"@

$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remoteScript))
& ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $RavenHost "powershell.exe -NoProfile -NonInteractive -EncodedCommand $encoded"
$restartExit = $LASTEXITCODE
if ($restartExit -eq 0) {
  Start-Sleep -Seconds 2
  $healthScript = Join-Path $PSScriptRoot "health-muninn.ps1"
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $healthScript -RavenHost $RavenHost -MuninnExe $MuninnExe -StorePath $StorePath
}
exit $restartExit

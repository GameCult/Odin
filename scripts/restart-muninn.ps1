param(
  [string] $RavenHost = "raven",
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\muninn.telemetry.cc",
  [string] $LogRoot = "C:\Meta\Odin\logs\muninn"
)

$ErrorActionPreference = "Stop"

$remoteScript = @"
`$ErrorActionPreference = "Stop"
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
`$lines = @(
  "@echo off",
  "cd /d ""`$muninnDir""",
  """$MuninnExe"" serve --store ""$StorePath"" --log-root ""$LogRoot"" --host raven --interval-seconds 15 1>>""$LogRoot\muninn-serve.out.log"" 2>>""$LogRoot\muninn-serve.err.log"""
)
Set-Content -LiteralPath `$launcher -Value `$lines -Encoding ASCII
cmd /c "schtasks /Delete /TN GameCult-Muninn /F 2>NUL"
cmd /c schtasks /Create /TN GameCult-Muninn /SC ONCE /ST 23:59 /TR `$launcher /IT /RL HIGHEST /F
cmd /c schtasks /Run /TN GameCult-Muninn
"@

$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remoteScript))
& ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $RavenHost "powershell.exe -NoProfile -EncodedCommand $encoded"
exit $LASTEXITCODE

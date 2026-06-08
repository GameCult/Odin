param(
  [string] $RavenHost = "raven",
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\muninn.telemetry.cc",
  [string] $ObsCatalogPath = "C:\Meta\Odin\state\muninn-obs-streams.tsv",
  [string] $LoopbackScript = "C:\Meta\Odin\Muninn\scripts\wasapi-loopback-capture.ps1",
  [string] $LogRoot = "C:\Meta\Odin\logs\muninn",
  [string] $Ffmpeg = "C:\Users\Madman's Lullaby\AppData\Local\Microsoft\WinGet\Links\ffmpeg.exe",
  [string] $TargetHost = "10.77.0.2",
  [int] $Port = 5200,
  [string] $ObsTargetHost = "10.77.0.2",
  [int] $ObsPort = 5204,
  [string] $AudioDevice = "Realtek"
)

$ErrorActionPreference = "Stop"

$remoteScript = @"
`$ErrorActionPreference = "Stop"
if (-not (Test-Path -LiteralPath "$MuninnExe")) {
  throw "Muninn executable not found at $MuninnExe"
}
if (-not (Test-Path -LiteralPath "$LoopbackScript")) {
  throw "Muninn loopback script not found at $LoopbackScript"
}
New-Item -ItemType Directory -Force -Path "$LogRoot" | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path -Parent "$StorePath") | Out-Null
Get-CimInstance Win32_Process |
  Where-Object { (`$_.Name -ieq "muninn.exe" -and `$_.CommandLine -like "*activate*") -or (`$_.Name -ieq "ffmpeg.exe" -and `$_.CommandLine -like "*srt://$TargetHost*") -or (`$_.Name -ieq "powershell.exe" -and `$_.CommandLine -like "*wasapi-loopback-capture.ps1*") } |
  ForEach-Object { taskkill.exe /PID `$_.ProcessId /T /F | Out-Null }
`$muninnDir = Split-Path -Parent "$MuninnExe"
`$launcher = Join-Path `$muninnDir "activate-raven-av-srt.cmd"
`$lines = @(
  "@echo off",
  "cd /d ""`$muninnDir""",
  """$MuninnExe"" activate --store ""$StorePath"" --obs-catalog ""$ObsCatalogPath"" --host raven --stream muninn.raven.av.srt --target-host ""$TargetHost"" --port $Port --obs-target-host ""$ObsTargetHost"" --obs-port $ObsPort --audio-device ""$AudioDevice"" --ffmpeg ""$Ffmpeg"" --loopback-script ""$LoopbackScript"" --log-root ""$LogRoot"" 1>>""$LogRoot\muninn-activate.out.log"" 2>>""$LogRoot\muninn-activate.err.log"""
)
Set-Content -LiteralPath `$launcher -Value `$lines -Encoding ASCII
cmd /c "schtasks /Delete /TN GameCult-Muninn-Activate /F 2>NUL"
cmd /c schtasks /Create /TN GameCult-Muninn-Activate /SC ONCE /ST 23:59 /TR `$launcher /IT /RL HIGHEST /F
cmd /c schtasks /Run /TN GameCult-Muninn-Activate
"@

$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remoteScript))
& ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $RavenHost "powershell.exe -NoProfile -EncodedCommand $encoded"
exit $LASTEXITCODE

param(
  [string] $RavenHost = "raven",
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\muninn.telemetry.cc",
  [string] $LoopbackScript = "C:\Meta\Odin\Muninn\scripts\wasapi-loopback-capture.ps1",
  [string] $LogRoot = "C:\Meta\Odin\logs\muninn",
  [string] $Ffmpeg = "C:\Users\Madman's Lullaby\AppData\Local\Microsoft\WinGet\Links\ffmpeg.exe",
  [string] $TargetHost = "10.77.0.2",
  [int] $Port = 5200,
  [string] $ObsTargetHost = "10.77.0.2",
  [int] $ObsPort = 5204,
  [string] $AudioDevice = "Realtek",
  [switch] $NoObsTarget
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
`$psLauncher = Join-Path `$muninnDir "activate-raven-av-srt.ps1"
`$pidPath = Join-Path "$LogRoot" "muninn-activate.pid"
`$obsArgs = ""
if (-not [bool]::Parse("$($NoObsTarget.IsPresent)") -and -not ("$TargetHost" -eq "$ObsTargetHost" -and $Port -eq $ObsPort)) {
  `$obsArgs = " --obs-target-host ""$ObsTargetHost"" --obs-port $ObsPort"
} else {
  `$obsArgs = " --no-obs-target"
}
`$arguments = @("activate", "--store", "$StorePath", "--host", "raven", "--stream", "muninn.raven.av.srt", "--target-host", "$TargetHost", "--port", "$Port")
if (`$obsArgs -like " --no-obs-target") {
  `$arguments += "--no-obs-target"
} else {
  `$arguments += @("--obs-target-host", "$ObsTargetHost", "--obs-port", "$ObsPort")
}
`$arguments += @("--audio-device", "$AudioDevice", "--ffmpeg", "$Ffmpeg", "--loopback-script", "$LoopbackScript", "--log-root", "$LogRoot")
`$encodedArguments = (`$arguments | ConvertTo-Json -Compress)
`$psLines = @(
  "`$ErrorActionPreference = ""Stop""",
  "`$arguments = '$encodedArguments' | ConvertFrom-Json",
  "`$process = Start-Process -FilePath ""$MuninnExe"" -ArgumentList `$arguments -WorkingDirectory ""`$muninnDir"" -WindowStyle Hidden -PassThru -RedirectStandardOutput ""$LogRoot\muninn-activate.out.log"" -RedirectStandardError ""$LogRoot\muninn-activate.err.log""",
  "`$process.Id | Set-Content -Encoding ASCII -LiteralPath ""`$pidPath"""
)
Set-Content -LiteralPath `$psLauncher -Value `$psLines -Encoding ASCII
`$lines = @(
  "@echo off",
  "cd /d ""`$muninnDir""",
  "powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File ""`$psLauncher"""
)
Set-Content -LiteralPath `$launcher -Value `$lines -Encoding ASCII
cmd /c "schtasks /Delete /TN GameCult-Muninn-Activate /F 2>NUL"
cmd /c schtasks /Create /TN GameCult-Muninn-Activate /SC ONCE /ST 23:59 /TR `$launcher /RL HIGHEST /F
cmd /c schtasks /Run /TN GameCult-Muninn-Activate
"@

$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remoteScript))
& ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $RavenHost "powershell.exe -NoProfile -EncodedCommand $encoded"
exit $LASTEXITCODE

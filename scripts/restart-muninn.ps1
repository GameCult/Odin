param(
  [string] $RavenHost = "raven",
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\muninn.telemetry.cc",
  [string] $LogRoot = "C:\Meta\Odin\logs\muninn",
  [string] $LoopbackScript = "C:\Meta\Odin\Muninn\scripts\wasapi-loopback-capture.ps1",
  [string] $Ffmpeg = "C:\Users\Madman's Lullaby\AppData\Local\Microsoft\WinGet\Links\ffmpeg.exe",
  [string] $TargetHost = "10.77.0.2",
  [int] $Port = 5200,
  [string] $ObsTargetHost = "10.77.0.2",
  [int] $ObsPort = 5204,
  [string] $AudioDevice = "Realtek",
  [string] $IdunnRudpHealth = "10.77.0.2:17870",
  [string] $IdunnDaemon = "muninn",
  [string] $IdunnHealthContract = "muninn.cultnet-rudp-remote-telemetry-health"
)

$ErrorActionPreference = "Stop"

function Set-AsciiFile {
  param(
    [Parameter(Mandatory = $true)] [string] $Path,
    [Parameter(Mandatory = $true)] [string] $Content
  )

  [System.IO.File]::WriteAllText($Path, ($Content -replace "`r?`n", "`r`n"), [System.Text.Encoding]::ASCII)
}

function ConvertTo-PowerShellStringLiteral {
  param(
    [Parameter(Mandatory = $true)] [string] $Value
  )

  return "'" + $Value.Replace("'", "''") + "'"
}

function ConvertTo-PowerShellArrayLiteral {
  param(
    [Parameter(Mandatory = $true)] [string[]] $Values
  )

  $lines = $Values | ForEach-Object { "  {0}" -f (ConvertTo-PowerShellStringLiteral $_) }
  return "@(`r`n{0}`r`n)" -f ($lines -join ",`r`n")
}

function New-HiddenPowerShellVbsLauncherContent {
  param(
    [Parameter(Mandatory = $true)] [string] $PsPath
  )

  return (@'
Set fso = CreateObject("Scripting.FileSystemObject")
scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)
psLauncher = "{0}"
Set shell = CreateObject("WScript.Shell")
shell.CurrentDirectory = scriptDir
shell.Run "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File """ & psLauncher & """", 0, False
'@) -f $PsPath
}

function New-WscriptCmdLauncherContent {
  param(
    [Parameter(Mandatory = $true)] [string] $WorkingDirectory,
    [Parameter(Mandatory = $true)] [string] $VbsPath
  )

  return (@'
@echo off
cd /d "{0}"
wscript.exe //B //Nologo "{1}"
'@) -f $WorkingDirectory, $VbsPath
}

function Invoke-RavenUploadedPowerShell {
  param(
    [Parameter(Mandatory = $true)] [string] $RavenHost,
    [Parameter(Mandatory = $true)] [string] $RemoteScriptContent,
    [Parameter(Mandatory = $true)] [object[]] $UploadSpecs,
    [Parameter(Mandatory = $true)] [string] $TempPrefix
  )

  $uploadId = [guid]::NewGuid().ToString("N")
  $localTempRoot = Join-Path $env:TEMP "$TempPrefix-$uploadId"
  $localRemoteScript = Join-Path $localTempRoot "$TempPrefix-$uploadId.ps1"
  $localSftpBatch = Join-Path $localTempRoot "$TempPrefix-$uploadId.sftp"
  $remoteSftpPath = "C:/Windows/Temp/$TempPrefix-$uploadId.ps1"
  $remotePsPath = "C:\Windows\Temp\$TempPrefix-$uploadId.ps1"

  try {
    New-Item -ItemType Directory -Force -Path $localTempRoot | Out-Null
    Set-AsciiFile -Path $localRemoteScript -Content $RemoteScriptContent

    $batchLines = @()
    foreach ($spec in $UploadSpecs) {
      $batchLines += 'put "{0}" "{1}"' -f $spec.LocalPath, $spec.RemotePath
    }
    $batchLines += 'put "{0}" "{1}"' -f $localRemoteScript, $remoteSftpPath
    Set-AsciiFile -Path $localSftpBatch -Content ($batchLines -join "`r`n")

    & sftp.exe -b $localSftpBatch $RavenHost
    if ($LASTEXITCODE -ne 0) {
      exit $LASTEXITCODE
    }

    $remoteRunner = @"
`$ErrorActionPreference = "Stop"
try {
  & "$remotePsPath"
  `$code = `$LASTEXITCODE
} finally {
  Remove-Item -LiteralPath "$remotePsPath" -Force -ErrorAction SilentlyContinue
}
exit `$code
"@
    $encodedRunner = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remoteRunner))
    & ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $RavenHost "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand $encodedRunner"
  } finally {
    Remove-Item -LiteralPath $localTempRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}

$muninnDir = Split-Path -Parent $MuninnExe
$serveCmdPath = Join-Path $muninnDir "start-muninn-serve.cmd"
$servePsPath = Join-Path $muninnDir "start-muninn-serve.ps1"
$serveVbsPath = Join-Path $muninnDir "start-muninn-serve-hidden.vbs"
$activateCmdPath = Join-Path $muninnDir "activate-raven-av-srt.cmd"
$activatePsPath = Join-Path $muninnDir "activate-raven-av-srt.ps1"
$activateVbsPath = Join-Path $muninnDir "activate-raven-av-srt-hidden.vbs"
$videoProofCmdPath = Join-Path $muninnDir "muninn-raven-video-to-starfire-obs.cmd"
$videoProofPsPath = Join-Path $muninnDir "muninn-raven-video-to-starfire-obs.ps1"
$videoProofVbsPath = Join-Path $muninnDir "muninn-raven-video-to-starfire-obs-hidden.vbs"

$serveArguments = @(
  "serve",
  "--store", $StorePath,
  "--log-root", $LogRoot,
  "--host", "raven",
  "--interval-seconds", "15",
  "--idunn-rudp-health", $IdunnRudpHealth,
  "--idunn-daemon", $IdunnDaemon,
  "--idunn-health-contract", $IdunnHealthContract
)

$activateArguments = @(
  "activate",
  "--store", $StorePath,
  "--host", "raven",
  "--stream", "muninn.raven.av.srt",
  "--target-host", $TargetHost,
  "--port", $Port.ToString()
)
if ($TargetHost -eq $ObsTargetHost -and $Port -eq $ObsPort) {
  $activateArguments += "--no-obs-target"
} else {
  $activateArguments += @("--obs-target-host", $ObsTargetHost, "--obs-port", $ObsPort.ToString())
}
$activateArguments += @(
  "--audio-device", $AudioDevice,
  "--ffmpeg", $Ffmpeg,
  "--loopback-script", $LoopbackScript,
  "--log-root", $LogRoot
)

$videoProofArguments = @(
  "-hide_banner",
  "-loglevel", "info",
  "-thread_queue_size", "1024",
  "-f", "lavfi",
  "-i", "ddagrab=framerate=30:output_idx=0:draw_mouse=1",
  "-thread_queue_size", "1024",
  "-f", "lavfi",
  "-i", "anullsrc=channel_layout=stereo:sample_rate=48000",
  "-map", "0:v:0",
  "-map", "1:a:0",
  "-c:v", "h264_nvenc",
  "-preset", "p4",
  "-tune", "ll",
  "-b:v", "12000k",
  "-maxrate", "12000k",
  "-bufsize", "24000k",
  "-g", "60",
  "-c:a", "aac",
  "-b:a", "192k",
  "-ar", "48000",
  "-ac", "2",
  "-f", "mpegts",
  ("srt://{0}:{1}?mode=caller&latency=120000&timeout=30000000" -f $ObsTargetHost, $ObsPort)
)

$servePsContent = (@'
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
New-Item -ItemType Directory -Force -Path (Split-Path -Parent {0}) | Out-Null
New-Item -ItemType Directory -Force -Path {1} | Out-Null
$arguments = {2}
$process = Start-Process -FilePath {3} -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput {4} -RedirectStandardError {5}
$process.Id | Set-Content -Encoding ASCII -LiteralPath {6}
'@) -f
  (ConvertTo-PowerShellStringLiteral $StorePath),
  (ConvertTo-PowerShellStringLiteral $LogRoot),
  (ConvertTo-PowerShellArrayLiteral $serveArguments),
  (ConvertTo-PowerShellStringLiteral $MuninnExe),
  (ConvertTo-PowerShellStringLiteral (Join-Path $LogRoot "muninn-serve.out.log")),
  (ConvertTo-PowerShellStringLiteral (Join-Path $LogRoot "muninn-serve.err.log")),
  (ConvertTo-PowerShellStringLiteral (Join-Path $LogRoot "muninn-serve.pid"))

$activatePsContent = (@'
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
New-Item -ItemType Directory -Force -Path (Split-Path -Parent {0}) | Out-Null
New-Item -ItemType Directory -Force -Path {1} | Out-Null
$arguments = {2}
$process = Start-Process -FilePath {3} -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput {4} -RedirectStandardError {5}
$process.Id | Set-Content -Encoding ASCII -LiteralPath {6}
'@) -f
  (ConvertTo-PowerShellStringLiteral $StorePath),
  (ConvertTo-PowerShellStringLiteral $LogRoot),
  (ConvertTo-PowerShellArrayLiteral $activateArguments),
  (ConvertTo-PowerShellStringLiteral $MuninnExe),
  (ConvertTo-PowerShellStringLiteral (Join-Path $LogRoot "muninn-activate.out.log")),
  (ConvertTo-PowerShellStringLiteral (Join-Path $LogRoot "muninn-activate.err.log")),
  (ConvertTo-PowerShellStringLiteral (Join-Path $LogRoot "muninn-activate.pid"))

$videoProofPsContent = (@'
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
New-Item -ItemType Directory -Force -Path {0} | Out-Null
$arguments = {1}
$process = Start-Process -FilePath {2} -ArgumentList $arguments -WorkingDirectory {3} -WindowStyle Hidden -PassThru -RedirectStandardOutput {4} -RedirectStandardError {5}
$process.Id | Set-Content -Encoding ASCII -LiteralPath {6}
'@) -f
  (ConvertTo-PowerShellStringLiteral $LogRoot),
  (ConvertTo-PowerShellArrayLiteral $videoProofArguments),
  (ConvertTo-PowerShellStringLiteral $Ffmpeg),
  (ConvertTo-PowerShellStringLiteral $muninnDir),
  (ConvertTo-PowerShellStringLiteral (Join-Path $LogRoot "muninn-video-proof.out.log")),
  (ConvertTo-PowerShellStringLiteral (Join-Path $LogRoot "muninn-video-proof.err.log")),
  (ConvertTo-PowerShellStringLiteral (Join-Path $LogRoot "muninn-video-proof.pid"))

$uploadRoot = Join-Path $env:TEMP ("odin-raven-muninn-launchers-" + [guid]::NewGuid().ToString("N"))

try {
  New-Item -ItemType Directory -Force -Path $uploadRoot | Out-Null

  $localServePs = Join-Path $uploadRoot "start-muninn-serve.ps1"
  $localServeVbs = Join-Path $uploadRoot "start-muninn-serve-hidden.vbs"
  $localServeCmd = Join-Path $uploadRoot "start-muninn-serve.cmd"
  $localActivatePs = Join-Path $uploadRoot "activate-raven-av-srt.ps1"
  $localActivateVbs = Join-Path $uploadRoot "activate-raven-av-srt-hidden.vbs"
  $localActivateCmd = Join-Path $uploadRoot "activate-raven-av-srt.cmd"
  $localVideoProofPs = Join-Path $uploadRoot "muninn-raven-video-to-starfire-obs.ps1"
  $localVideoProofVbs = Join-Path $uploadRoot "muninn-raven-video-to-starfire-obs-hidden.vbs"
  $localVideoProofCmd = Join-Path $uploadRoot "muninn-raven-video-to-starfire-obs.cmd"

  Set-AsciiFile -Path $localServePs -Content $servePsContent
  Set-AsciiFile -Path $localServeVbs -Content (New-HiddenPowerShellVbsLauncherContent -PsPath $servePsPath)
  Set-AsciiFile -Path $localServeCmd -Content (New-WscriptCmdLauncherContent -WorkingDirectory $muninnDir -VbsPath $serveVbsPath)
  Set-AsciiFile -Path $localActivatePs -Content $activatePsContent
  Set-AsciiFile -Path $localActivateVbs -Content (New-HiddenPowerShellVbsLauncherContent -PsPath $activatePsPath)
  Set-AsciiFile -Path $localActivateCmd -Content (New-WscriptCmdLauncherContent -WorkingDirectory $muninnDir -VbsPath $activateVbsPath)
  Set-AsciiFile -Path $localVideoProofPs -Content $videoProofPsContent
  Set-AsciiFile -Path $localVideoProofVbs -Content (New-HiddenPowerShellVbsLauncherContent -PsPath $videoProofPsPath)
  Set-AsciiFile -Path $localVideoProofCmd -Content (New-WscriptCmdLauncherContent -WorkingDirectory $muninnDir -VbsPath $videoProofVbsPath)

  $remoteScript = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"

if (-not (Test-Path -LiteralPath "$MuninnExe")) {
  throw "Muninn executable not found at $MuninnExe"
}
if (-not (Test-Path -LiteralPath "$LoopbackScript")) {
  throw "Muninn loopback script not found at $LoopbackScript"
}
if (-not (Test-Path -LiteralPath "$Ffmpeg")) {
  throw "FFmpeg executable not found at $Ffmpeg"
}
foreach (`$path in @(
  "$servePsPath",
  "$serveVbsPath",
  "$activatePsPath",
  "$activateVbsPath",
  "$videoProofPsPath",
  "$videoProofVbsPath"
)) {
  if (-not (Test-Path -LiteralPath `$path)) {
    throw "Required launcher path not found at `$path"
  }
}

Get-CimInstance Win32_Process |
  Where-Object { `$_.Name -ieq "muninn.exe" -and `$_.CommandLine -like "*serve*" } |
  ForEach-Object {
    & taskkill.exe /PID `$_.ProcessId /T /F | Out-Null
  }

New-Item -ItemType Directory -Force -Path "$LogRoot" | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path -Parent "$StorePath") | Out-Null

function Register-HiddenVbsTask {
  param(
    [Parameter(Mandatory = `$true)] [string] `$TaskName,
    [Parameter(Mandatory = `$true)] [string] `$VbsPath
  )

  `$taskAction = New-ScheduledTaskAction -Execute "wscript.exe" -Argument "//B //Nologo ""`$VbsPath"""
  `$taskTrigger = New-ScheduledTaskTrigger -Once -At ([DateTime]::Today.AddHours(23).AddMinutes(59))
  `$taskPrincipal = New-ScheduledTaskPrincipal -UserId ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
  `$taskSettings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew
  Register-ScheduledTask -TaskName `$TaskName -Action `$taskAction -Trigger `$taskTrigger -Principal `$taskPrincipal -Settings `$taskSettings -Force | Out-Null
}

function Assert-HiddenVbsTask {
  param(
    [Parameter(Mandatory = `$true)] [string] `$TaskName,
    [Parameter(Mandatory = `$true)] [string] `$VbsPath,
    [Parameter(Mandatory = `$true)] [string] `$PsPath
  )

  `$task = Get-ScheduledTask -TaskName `$TaskName -ErrorAction Stop
  `$action = @(`$task.Actions)[0]
  if (`$action.Execute -notmatch '(^|\\)wscript\.exe$') {
    throw "`$TaskName action executes `$(`$action.Execute), expected wscript.exe"
  }
  if (`$action.Arguments -notlike "*`$VbsPath*") {
    throw "`$TaskName action arguments `$(`$action.Arguments) do not reference `$VbsPath"
  }
  if (`$action.Arguments -notlike "*//B*" -or `$action.Arguments -notlike "*//Nologo*") {
    throw "`$TaskName action arguments `$(`$action.Arguments) do not force background WScript execution"
  }
  if (-not (Test-Path -LiteralPath `$PsPath)) {
    throw "`$TaskName PowerShell launcher not found at `$PsPath"
  }
  `$vbs = Get-Content -LiteralPath `$VbsPath -Raw
  if (`$vbs -match 'cmdPath\s*=') {
    throw "`$TaskName hidden launcher at `$VbsPath still routes through a cmdPath trampoline"
  }
  if (`$vbs -notmatch '\.ps1') {
    throw "`$TaskName hidden launcher at `$VbsPath does not reference a PowerShell launcher"
  }
}

Register-HiddenVbsTask -TaskName "GameCult-Muninn" -VbsPath "$serveVbsPath"
Register-HiddenVbsTask -TaskName "GameCult-Muninn-Activate" -VbsPath "$activateVbsPath"
Register-HiddenVbsTask -TaskName "GameCult-Muninn-VideoProof" -VbsPath "$videoProofVbsPath"

Assert-HiddenVbsTask -TaskName "GameCult-Muninn" -VbsPath "$serveVbsPath" -PsPath "$servePsPath"
Assert-HiddenVbsTask -TaskName "GameCult-Muninn-Activate" -VbsPath "$activateVbsPath" -PsPath "$activatePsPath"
Assert-HiddenVbsTask -TaskName "GameCult-Muninn-VideoProof" -VbsPath "$videoProofVbsPath" -PsPath "$videoProofPsPath"

Start-ScheduledTask -TaskName "GameCult-Muninn"
Start-Sleep -Seconds 2

`$process = Get-CimInstance Win32_Process |
  Where-Object { `$_.Name -ieq "muninn.exe" -and `$_.CommandLine -like "*serve*" -and `$_.CommandLine -like "*--host*raven*" } |
  Select-Object -First 1
if (`$null -eq `$process) {
  throw "Muninn serve process is not running on Raven"
}
foreach (`$pattern in @(
  "--idunn-rudp-health",
  "$IdunnRudpHealth",
  "--idunn-daemon",
  "$IdunnDaemon",
  "--idunn-health-contract",
  "$IdunnHealthContract"
)) {
  if (`$process.CommandLine -notlike "*`$pattern*") {
    throw "Muninn Raven serve command line is missing `${pattern}: `$(`$process.CommandLine)"
  }
}
"@

  $uploadSpecs = @(
    @{ LocalPath = $localServePs; RemotePath = ($servePsPath -replace "\\", "/") },
    @{ LocalPath = $localServeVbs; RemotePath = ($serveVbsPath -replace "\\", "/") },
    @{ LocalPath = $localServeCmd; RemotePath = ($serveCmdPath -replace "\\", "/") },
    @{ LocalPath = $localActivatePs; RemotePath = ($activatePsPath -replace "\\", "/") },
    @{ LocalPath = $localActivateVbs; RemotePath = ($activateVbsPath -replace "\\", "/") },
    @{ LocalPath = $localActivateCmd; RemotePath = ($activateCmdPath -replace "\\", "/") },
    @{ LocalPath = $localVideoProofPs; RemotePath = ($videoProofPsPath -replace "\\", "/") },
    @{ LocalPath = $localVideoProofVbs; RemotePath = ($videoProofVbsPath -replace "\\", "/") },
    @{ LocalPath = $localVideoProofCmd; RemotePath = ($videoProofCmdPath -replace "\\", "/") }
  )

  Invoke-RavenUploadedPowerShell -RavenHost $RavenHost -RemoteScriptContent $remoteScript -UploadSpecs $uploadSpecs -TempPrefix "odin-raven-muninn-restart"
} finally {
  Remove-Item -LiteralPath $uploadRoot -Recurse -Force -ErrorAction SilentlyContinue
}

$restartExit = $LASTEXITCODE
if ($restartExit -eq 0) {
  Start-Sleep -Seconds 2
  $healthScript = Join-Path $PSScriptRoot "health-muninn.ps1"
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $healthScript -RavenHost $RavenHost -MuninnExe $MuninnExe -StorePath $StorePath
}
exit $restartExit

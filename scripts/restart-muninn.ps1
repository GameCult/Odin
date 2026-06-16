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
`$activateCmd = Join-Path `$muninnDir "activate-raven-av-srt.cmd"
`$activateVbs = Join-Path `$muninnDir "activate-raven-av-srt-hidden.vbs"
`$videoProofCmd = Join-Path `$muninnDir "muninn-raven-video-to-starfire-obs.cmd"
`$videoProofVbs = Join-Path `$muninnDir "muninn-raven-video-to-starfire-obs-hidden.vbs"
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

function Write-HiddenCmdLauncher {
  param(
    [Parameter(Mandatory = `$true)] [string] `$CmdPath,
    [Parameter(Mandatory = `$true)] [string] `$VbsPath
  )

  if (-not (Test-Path -LiteralPath `$CmdPath)) {
    return
  }

  `$launcherLines = @(
    'Set fso = CreateObject("Scripting.FileSystemObject")',
    'Set shell = CreateObject("WScript.Shell")',
    "cmdPath = ""`$CmdPath""",
    'shell.CurrentDirectory = fso.GetParentFolderName(cmdPath)',
    'shell.Run """" & cmdPath & """", 0, False'
  )
  Set-Content -LiteralPath `$VbsPath -Value `$launcherLines -Encoding ASCII
}

function Register-HiddenVbsTask {
  param(
    [Parameter(Mandatory = `$true)] [string] `$TaskName,
    [Parameter(Mandatory = `$true)] [string] `$VbsPath
  )

  if (-not (Test-Path -LiteralPath `$VbsPath)) {
    return
  }

  `$taskAction = New-ScheduledTaskAction -Execute "wscript.exe" -Argument "//B //Nologo ""`$VbsPath"""
  `$taskTrigger = New-ScheduledTaskTrigger -Once -At ([DateTime]::Today.AddHours(23).AddMinutes(59))
  `$taskPrincipal = New-ScheduledTaskPrincipal -UserId ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
  `$taskSettings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew
  Register-ScheduledTask -TaskName `$TaskName -Action `$taskAction -Trigger `$taskTrigger -Principal `$taskPrincipal -Settings `$taskSettings -Force | Out-Null
}

function Assert-HiddenVbsTask {
  param(
    [Parameter(Mandatory = `$true)] [string] `$TaskName,
    [Parameter(Mandatory = `$true)] [string] `$VbsPath
  )

  if (-not (Test-Path -LiteralPath `$VbsPath)) {
    return
  }

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
}

Write-HiddenCmdLauncher -CmdPath `$activateCmd -VbsPath `$activateVbs
Write-HiddenCmdLauncher -CmdPath `$videoProofCmd -VbsPath `$videoProofVbs
Register-HiddenVbsTask -TaskName "GameCult-Muninn" -VbsPath `$vbsLauncher
Register-HiddenVbsTask -TaskName "GameCult-Muninn-Activate" -VbsPath `$activateVbs
Register-HiddenVbsTask -TaskName "GameCult-Muninn-VideoProof" -VbsPath `$videoProofVbs
Assert-HiddenVbsTask -TaskName "GameCult-Muninn" -VbsPath `$vbsLauncher
Assert-HiddenVbsTask -TaskName "GameCult-Muninn-Activate" -VbsPath `$activateVbs
Assert-HiddenVbsTask -TaskName "GameCult-Muninn-VideoProof" -VbsPath `$videoProofVbs
Start-ScheduledTask -TaskName "GameCult-Muninn"
"@

$uploadId = [guid]::NewGuid().ToString("N")
$localRemoteScript = Join-Path $env:TEMP "odin-raven-muninn-restart-$uploadId.ps1"
$localSftpBatch = Join-Path $env:TEMP "odin-raven-muninn-restart-$uploadId.sftp"
$remoteSftpPath = "C:/Windows/Temp/odin-raven-muninn-restart-$uploadId.ps1"
$remotePsPath = "C:\Windows\Temp\odin-raven-muninn-restart-$uploadId.ps1"
try {
  Set-Content -LiteralPath $localRemoteScript -Encoding ASCII -Value $remoteScript
  Set-Content -LiteralPath $localSftpBatch -Encoding ASCII -Value "put ""$localRemoteScript"" ""$remoteSftpPath"""
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
  Remove-Item -LiteralPath $localRemoteScript, $localSftpBatch -Force -ErrorAction SilentlyContinue
}
$restartExit = $LASTEXITCODE
if ($restartExit -eq 0) {
  Start-Sleep -Seconds 2
  $healthScript = Join-Path $PSScriptRoot "health-muninn.ps1"
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $healthScript -RavenHost $RavenHost -MuninnExe $MuninnExe -StorePath $StorePath
}
exit $restartExit

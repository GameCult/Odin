param(
  [string] $RavenHost = "raven",
  [string] $MuninnDir = "C:\Meta\Odin\Muninn"
)

$ErrorActionPreference = "Stop"

$remoteScript = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"

function Register-HiddenVbsTask {
  param(
    [Parameter(Mandatory = `$true)] [string] `$TaskName,
    [Parameter(Mandatory = `$true)] [string] `$VbsPath
  )

  if (-not (Test-Path -LiteralPath `$VbsPath)) {
    throw "Hidden launcher not found for `$TaskName at `$VbsPath"
  }

  `$action = New-ScheduledTaskAction -Execute "wscript.exe" -Argument "//B //Nologo ""`$VbsPath"""
  `$trigger = New-ScheduledTaskTrigger -Once -At ([DateTime]::Today.AddHours(23).AddMinutes(59))
  `$principal = New-ScheduledTaskPrincipal -UserId ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
  `$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew
  Register-ScheduledTask -TaskName `$TaskName -Action `$action -Trigger `$trigger -Principal `$principal -Settings `$settings -Force | Out-Null
}

function Assert-HiddenVbsTask {
  param(
    [Parameter(Mandatory = `$true)] [string] `$TaskName,
    [Parameter(Mandatory = `$true)] [string] `$VbsPath
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
  `$vbs = Get-Content -LiteralPath `$VbsPath -Raw
  if (`$vbs -match 'cmdPath\s*=') {
    throw "`$TaskName hidden launcher at `$VbsPath still routes through a cmdPath trampoline"
  }
  if (`$vbs -notmatch '\.ps1') {
    throw "`$TaskName hidden launcher at `$VbsPath does not reference a PowerShell launcher"
  }
}

`$serveVbs = Join-Path "$MuninnDir" "start-muninn-serve-hidden.vbs"
`$activateCmd = Join-Path "$MuninnDir" "activate-raven-av-srt.cmd"
`$activatePs = Join-Path "$MuninnDir" "activate-raven-av-srt.ps1"
`$activateVbs = Join-Path "$MuninnDir" "activate-raven-av-srt-hidden.vbs"
`$videoProofCmd = Join-Path "$MuninnDir" "muninn-raven-video-to-starfire-obs.cmd"
`$videoProofPs = Join-Path "$MuninnDir" "muninn-raven-video-to-starfire-obs.ps1"
`$videoProofVbs = Join-Path "$MuninnDir" "muninn-raven-video-to-starfire-obs-hidden.vbs"

function Write-HiddenPowerShellVbsLauncher {
  param(
    [Parameter(Mandatory = `$true)] [string] `$PsPath,
    [Parameter(Mandatory = `$true)] [string] `$VbsPath
  )

  if (-not (Test-Path -LiteralPath `$PsPath)) {
    throw "PowerShell launcher not found at `$PsPath"
  }

  `$lines = @(
    'Set fso = CreateObject("Scripting.FileSystemObject")',
    'scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)',
    'Set shell = CreateObject("WScript.Shell")',
    "psLauncher = ""`$PsPath""",
    'shell.CurrentDirectory = scriptDir',
    'shell.Run "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File """ & psLauncher & """", 0, False'
  )
  Set-Content -LiteralPath `$VbsPath -Value `$lines -Encoding ASCII
}

function Write-WscriptCmdLauncher {
  param(
    [Parameter(Mandatory = `$true)] [string] `$CmdPath,
    [Parameter(Mandatory = `$true)] [string] `$WorkingDirectory,
    [Parameter(Mandatory = `$true)] [string] `$VbsPath
  )

  `$cmdLines = @(
    '@echo off',
    "cd /d ""`$WorkingDirectory""",
    "wscript.exe //B //Nologo ""`$VbsPath"""
  )
  Set-Content -LiteralPath `$CmdPath -Value `$cmdLines -Encoding ASCII
}

if (Test-Path -LiteralPath `$activatePs) {
  Write-HiddenPowerShellVbsLauncher -PsPath `$activatePs -VbsPath `$activateVbs
  if (Test-Path -LiteralPath `$activateCmd) {
    Write-WscriptCmdLauncher -CmdPath `$activateCmd -WorkingDirectory "$MuninnDir" -VbsPath `$activateVbs
  }
}
if (Test-Path -LiteralPath `$videoProofPs) {
  Write-HiddenPowerShellVbsLauncher -PsPath `$videoProofPs -VbsPath `$videoProofVbs
  if (Test-Path -LiteralPath `$videoProofCmd) {
    Write-WscriptCmdLauncher -CmdPath `$videoProofCmd -WorkingDirectory "$MuninnDir" -VbsPath `$videoProofVbs
  }
}

Register-HiddenVbsTask -TaskName "GameCult-Muninn" -VbsPath `$serveVbs
if (Test-Path -LiteralPath `$activateVbs) {
  Register-HiddenVbsTask -TaskName "GameCult-Muninn-Activate" -VbsPath `$activateVbs
}
if (Test-Path -LiteralPath `$videoProofVbs) {
  Register-HiddenVbsTask -TaskName "GameCult-Muninn-VideoProof" -VbsPath `$videoProofVbs
}

Assert-HiddenVbsTask -TaskName "GameCult-Muninn" -VbsPath `$serveVbs
if (Test-Path -LiteralPath `$activateVbs) {
  Assert-HiddenVbsTask -TaskName "GameCult-Muninn-Activate" -VbsPath `$activateVbs
}
if (Test-Path -LiteralPath `$videoProofVbs) {
  Assert-HiddenVbsTask -TaskName "GameCult-Muninn-VideoProof" -VbsPath `$videoProofVbs
}
"@

$uploadId = [guid]::NewGuid().ToString("N")
$localRemoteScript = Join-Path $env:TEMP "odin-raven-muninn-task-repair-$uploadId.ps1"
$localSftpBatch = Join-Path $env:TEMP "odin-raven-muninn-task-repair-$uploadId.sftp"
$remoteSftpPath = "C:/Windows/Temp/odin-raven-muninn-task-repair-$uploadId.ps1"
$remotePsPath = "C:\Windows\Temp\odin-raven-muninn-task-repair-$uploadId.ps1"
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
exit $LASTEXITCODE

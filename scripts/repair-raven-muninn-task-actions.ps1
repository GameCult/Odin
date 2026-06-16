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

`$serveVbs = Join-Path "$MuninnDir" "start-muninn-serve-hidden.vbs"
`$activateCmd = Join-Path "$MuninnDir" "activate-raven-av-srt.cmd"
`$activateVbs = Join-Path "$MuninnDir" "activate-raven-av-srt-hidden.vbs"
`$videoProofCmd = Join-Path "$MuninnDir" "muninn-raven-video-to-starfire-obs.cmd"
`$videoProofVbs = Join-Path "$MuninnDir" "muninn-raven-video-to-starfire-obs-hidden.vbs"

function Write-HiddenCmdLauncher {
  param(
    [Parameter(Mandatory = `$true)] [string] `$CmdPath,
    [Parameter(Mandatory = `$true)] [string] `$VbsPath
  )

  if (-not (Test-Path -LiteralPath `$CmdPath)) {
    throw "Command launcher not found at `$CmdPath"
  }

  `$lines = @(
    'Set shell = CreateObject("WScript.Shell")',
    "cmdPath = ""`$CmdPath""",
    'shell.Run """" & cmdPath & """", 0, False'
  )
  Set-Content -LiteralPath `$VbsPath -Value `$lines -Encoding ASCII
}

if ((Test-Path -LiteralPath `$activateCmd) -and -not (Test-Path -LiteralPath `$activateVbs)) {
  Write-HiddenCmdLauncher -CmdPath `$activateCmd -VbsPath `$activateVbs
}
if ((Test-Path -LiteralPath `$videoProofCmd) -and -not (Test-Path -LiteralPath `$videoProofVbs)) {
  Write-HiddenCmdLauncher -CmdPath `$videoProofCmd -VbsPath `$videoProofVbs
}

Register-HiddenVbsTask -TaskName "GameCult-Muninn" -VbsPath `$serveVbs
if (Test-Path -LiteralPath `$activateVbs) {
  Register-HiddenVbsTask -TaskName "GameCult-Muninn-Activate" -VbsPath `$activateVbs
}
if (Test-Path -LiteralPath `$videoProofVbs) {
  Register-HiddenVbsTask -TaskName "GameCult-Muninn-VideoProof" -VbsPath `$videoProofVbs
}
"@

$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remoteScript))
& ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $RavenHost "powershell.exe -NoProfile -NonInteractive -EncodedCommand $encoded"
exit $LASTEXITCODE

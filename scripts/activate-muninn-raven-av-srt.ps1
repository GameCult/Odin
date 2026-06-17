param(
  [string] $RavenHost = "raven",
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\muninn.activate.cc",
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
$activateCmdPath = Join-Path $muninnDir "activate-raven-av-srt.cmd"
$activatePsPath = Join-Path $muninnDir "activate-raven-av-srt.ps1"
$activateVbsPath = Join-Path $muninnDir "activate-raven-av-srt-hidden.vbs"

$activateArguments = @(
  "activate",
  "--store", $StorePath,
  "--host", "raven",
  "--stream", "muninn.raven.av.srt",
  "--target-host", $TargetHost,
  "--port", $Port.ToString()
)
if ($NoObsTarget.IsPresent -or ($TargetHost -eq $ObsTargetHost -and $Port -eq $ObsPort)) {
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

$activatePsContent = (@'
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
New-Item -ItemType Directory -Force -Path (Split-Path -Parent {0}) | Out-Null
New-Item -ItemType Directory -Force -Path {1} | Out-Null
$arguments = {2}
function Quote-NativeArgument([string] $Value) {{
  if ($Value -match '[\s"]') {{
    return '"' + $Value.Replace('"', '\"') + '"'
  }}
  return $Value
}}
$argumentLine = ($arguments | ForEach-Object {{ Quote-NativeArgument $_ }}) -join ' '
$process = Start-Process -FilePath {3} -ArgumentList $argumentLine -WindowStyle Hidden -PassThru -RedirectStandardOutput {4} -RedirectStandardError {5}
$process.Id | Set-Content -Encoding ASCII -LiteralPath {6}
'@) -f
  (ConvertTo-PowerShellStringLiteral $StorePath),
  (ConvertTo-PowerShellStringLiteral $LogRoot),
  (ConvertTo-PowerShellArrayLiteral $activateArguments),
  (ConvertTo-PowerShellStringLiteral $MuninnExe),
  (ConvertTo-PowerShellStringLiteral (Join-Path $LogRoot "muninn-activate.out.log")),
  (ConvertTo-PowerShellStringLiteral (Join-Path $LogRoot "muninn-activate.err.log")),
  (ConvertTo-PowerShellStringLiteral (Join-Path $LogRoot "muninn-activate.pid"))

$uploadRoot = Join-Path $env:TEMP ("odin-raven-muninn-activate-" + [guid]::NewGuid().ToString("N"))

try {
  New-Item -ItemType Directory -Force -Path $uploadRoot | Out-Null

  $localActivatePs = Join-Path $uploadRoot "activate-raven-av-srt.ps1"
  $localActivateVbs = Join-Path $uploadRoot "activate-raven-av-srt-hidden.vbs"
  $localActivateCmd = Join-Path $uploadRoot "activate-raven-av-srt.cmd"

  Set-AsciiFile -Path $localActivatePs -Content $activatePsContent
  Set-AsciiFile -Path $localActivateVbs -Content (New-HiddenPowerShellVbsLauncherContent -PsPath $activatePsPath)
  Set-AsciiFile -Path $localActivateCmd -Content (New-WscriptCmdLauncherContent -WorkingDirectory $muninnDir -VbsPath $activateVbsPath)

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
  "$activatePsPath",
  "$activateVbsPath"
)) {
  if (-not (Test-Path -LiteralPath `$path)) {
    throw "Required launcher path not found at `$path"
  }
}

Get-CimInstance Win32_Process |
  Where-Object {
    (`$_.Name -ieq "muninn.exe" -and `$_.CommandLine -like "*activate*") -or
    (`$_.Name -ieq "ffmpeg.exe" -and `$_.CommandLine -like "*srt://$TargetHost*") -or
    (`$_.Name -ieq "powershell.exe" -and `$_.CommandLine -like "*wasapi-loopback-capture.ps1*")
  } |
  ForEach-Object {
    & taskkill.exe /PID `$_.ProcessId /T /F | Out-Null
  }

New-Item -ItemType Directory -Force -Path "$LogRoot" | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path -Parent "$StorePath") | Out-Null

`$taskAction = New-ScheduledTaskAction -Execute "wscript.exe" -Argument "//B //Nologo ""$activateVbsPath"""
`$taskTrigger = New-ScheduledTaskTrigger -Once -At ([DateTime]::Today.AddHours(23).AddMinutes(59))
`$taskPrincipal = New-ScheduledTaskPrincipal -UserId ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
`$taskSettings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName "GameCult-Muninn-Activate" -Action `$taskAction -Trigger `$taskTrigger -Principal `$taskPrincipal -Settings `$taskSettings -Force | Out-Null

`$task = Get-ScheduledTask -TaskName "GameCult-Muninn-Activate" -ErrorAction Stop
`$action = @(`$task.Actions)[0]
if (`$action.Execute -notmatch '(^|\\)wscript\.exe$') {
  throw "GameCult-Muninn-Activate action executes `$(`$action.Execute), expected wscript.exe"
}
if (`$action.Arguments -notlike "*$activateVbsPath*") {
  throw "GameCult-Muninn-Activate action arguments `$(`$action.Arguments) do not reference $activateVbsPath"
}
if (`$action.Arguments -notlike "*//B*" -or `$action.Arguments -notlike "*//Nologo*") {
  throw "GameCult-Muninn-Activate action arguments `$(`$action.Arguments) do not force background WScript execution"
}
`$vbs = Get-Content -LiteralPath "$activateVbsPath" -Raw
if (`$vbs -match 'cmdPath\s*=') {
  throw "GameCult-Muninn-Activate hidden launcher at $activateVbsPath still routes through a cmdPath trampoline"
}
if (`$vbs -notmatch '\.ps1') {
  throw "GameCult-Muninn-Activate hidden launcher at $activateVbsPath does not reference a PowerShell launcher"
}

Start-ScheduledTask -TaskName "GameCult-Muninn-Activate"
"@

  $uploadSpecs = @(
    @{ LocalPath = $localActivatePs; RemotePath = ($activatePsPath -replace "\\", "/") },
    @{ LocalPath = $localActivateVbs; RemotePath = ($activateVbsPath -replace "\\", "/") },
    @{ LocalPath = $localActivateCmd; RemotePath = ($activateCmdPath -replace "\\", "/") }
  )

  Invoke-RavenUploadedPowerShell -RavenHost $RavenHost -RemoteScriptContent $remoteScript -UploadSpecs $uploadSpecs -TempPrefix "odin-raven-muninn-activate"
} finally {
  Remove-Item -LiteralPath $uploadRoot -Recurse -Force -ErrorAction SilentlyContinue
}

exit $LASTEXITCODE

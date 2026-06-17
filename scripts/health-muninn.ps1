param(
  [string] $RavenHost = "raven",
  [string] $MuninnExe = "C:\Meta\Odin\Muninn\muninn.exe",
  [string] $StorePath = "C:\Meta\Odin\state\muninn.telemetry.cc",
  [string] $IdunnRudpHealth = "10.77.0.2:17870"
)

$ErrorActionPreference = "Stop"

function Set-AsciiFile {
  param(
    [Parameter(Mandatory = $true)] [string] $Path,
    [Parameter(Mandatory = $true)] [string] $Content
  )

  [System.IO.File]::WriteAllText($Path, ($Content -replace "`r?`n", "`r`n"), [System.Text.Encoding]::ASCII)
}

function Invoke-RavenUploadedPowerShell {
  param(
    [Parameter(Mandatory = $true)] [string] $RavenHost,
    [Parameter(Mandatory = $true)] [string] $RemoteScriptContent,
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
    Set-AsciiFile -Path $localSftpBatch -Content ('put "{0}" "{1}"' -f $localRemoteScript, $remoteSftpPath)

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

$remoteScript = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"
if (-not (Test-Path -LiteralPath "$MuninnExe")) {
  throw "Muninn executable not found at $MuninnExe"
}
`$muninnDir = Split-Path -Parent "$MuninnExe"
`$taskExpectations = @(
  @{ Name = "GameCult-Muninn"; Vbs = Join-Path `$muninnDir "start-muninn-serve-hidden.vbs"; Ps = Join-Path `$muninnDir "start-muninn-serve.ps1" },
  @{ Name = "GameCult-Muninn-Activate"; Vbs = Join-Path `$muninnDir "activate-raven-av-srt-hidden.vbs"; Ps = Join-Path `$muninnDir "activate-raven-av-srt.ps1" },
  @{ Name = "GameCult-Muninn-VideoProof"; Vbs = Join-Path `$muninnDir "muninn-raven-video-to-starfire-obs-hidden.vbs"; Ps = Join-Path `$muninnDir "muninn-raven-video-to-starfire-obs.ps1" }
)
foreach (`$expectation in `$taskExpectations) {
  `$task = Get-ScheduledTask -TaskName `$expectation.Name -ErrorAction Stop
  `$action = @(`$task.Actions)[0]
  if (`$action.Execute -notmatch '(^|\\)wscript\.exe$') {
    throw "`$(`$expectation.Name) action executes `$(`$action.Execute), expected wscript.exe"
  }
  if (`$action.Arguments -notlike "*`$(`$expectation.Vbs)*") {
    throw "`$(`$expectation.Name) action arguments `$(`$action.Arguments) do not reference `$(`$expectation.Vbs)"
  }
  if (`$action.Arguments -notlike "*//B*" -or `$action.Arguments -notlike "*//Nologo*") {
    throw "`$(`$expectation.Name) action arguments `$(`$action.Arguments) do not force hidden WScript execution"
  }
  if (-not (Test-Path -LiteralPath `$expectation.Vbs)) {
    throw "Missing hidden launcher for `$(`$expectation.Name) at `$(`$expectation.Vbs)"
  }
  if (-not (Test-Path -LiteralPath `$expectation.Ps)) {
    throw "Missing PowerShell launcher for `$(`$expectation.Name) at `$(`$expectation.Ps)"
  }
  `$psContent = Get-Content -LiteralPath `$expectation.Ps -Raw
  if (`$psContent -like "*'' | ConvertFrom-Json*") {
    throw "`$(`$expectation.Ps) has an empty encoded-arguments payload"
  }
  if (`$psContent -notmatch 'Start-Process') {
    throw "`$(`$expectation.Ps) does not launch a hidden process"
  }
  if (`$expectation.Name -eq "GameCult-Muninn-Activate" -and `$psContent -notmatch 'muninn\.exe') {
    throw "`$(`$expectation.Ps) does not launch muninn.exe"
  }
  if (`$expectation.Name -eq "GameCult-Muninn-VideoProof" -and `$psContent -notmatch 'ffmpeg\.exe') {
    throw "`$(`$expectation.Ps) does not launch ffmpeg.exe"
  }
  if (`$expectation.Name -eq "GameCult-Muninn-VideoProof" -and `$psContent -notmatch 'srt://.+:\d+\?mode=caller') {
    throw "`$(`$expectation.Ps) does not carry the expected SRT caller target"
  }
  `$vbs = Get-Content -LiteralPath `$expectation.Vbs -Raw
  if (`$vbs -match 'cmdPath\s*=') {
    throw "`$(`$expectation.Vbs) still routes through a cmdPath trampoline"
  }
  if (`$vbs -notmatch '\.ps1') {
    throw "`$(`$expectation.Vbs) does not reference a PowerShell launcher"
  }
}
`$process = Get-CimInstance Win32_Process |
  Where-Object { `$_.Name -ieq "muninn.exe" -and `$_.CommandLine -like "*serve*" } |
  Select-Object -First 1
if (`$null -eq `$process) {
  throw "Muninn serve process is not running"
}
foreach (`$pattern in @(
  "--host raven",
  "--idunn-rudp-health $IdunnRudpHealth",
  "--idunn-daemon muninn",
  "--idunn-health-contract muninn.cultnet-rudp-remote-telemetry-health"
)) {
  if (`$process.CommandLine -notlike "*`$pattern*") {
    throw "Muninn serve process is missing expected command-line segment `${pattern}: `$(`$process.CommandLine)"
  }
}
`$healthArgs = @(
  "--health",
  "--store", "$StorePath"
)
& "$MuninnExe" @healthArgs
"@

Invoke-RavenUploadedPowerShell -RavenHost $RavenHost -RemoteScriptContent $remoteScript -TempPrefix "odin-raven-muninn-health"
exit $LASTEXITCODE

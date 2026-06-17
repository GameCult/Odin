param(
  [string] $SshTarget = "nightwing",
  [string] $MuninnExe = "/home/metacrat/.local/bin/muninn",
  [string] $StorePath = "/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc",
  [string] $PidPath = "/home/metacrat/.local/state/gamecult/muninn/muninn.pid",
  [string[]] $MoveState = @(),
  [int] $IntervalSeconds = 15,
  [int] $MaxStoreAgeSeconds = 180,
  [string] $IdunnRudpHealth = "10.77.0.2:17870"
)

$ErrorActionPreference = "Stop"

function Set-AsciiFile {
  param(
    [Parameter(Mandatory = $true)] [string] $Path,
    [Parameter(Mandatory = $true)] [string] $Content
  )

  [System.IO.File]::WriteAllText($Path, ($Content -replace "`r?`n", "`n"), [System.Text.Encoding]::ASCII)
}

function Invoke-NightwingUploadedShell {
  param(
    [Parameter(Mandatory = $true)] [string] $SshTarget,
    [Parameter(Mandatory = $true)] [string] $RemoteScriptContent,
    [Parameter(Mandatory = $true)] [string] $TempPrefix
  )

  $uploadId = [guid]::NewGuid().ToString("N")
  $localTempRoot = Join-Path $env:TEMP "$TempPrefix-$uploadId"
  $localRemoteScript = Join-Path $localTempRoot "$TempPrefix-$uploadId.sh"
  $localSftpBatch = Join-Path $localTempRoot "$TempPrefix-$uploadId.sftp"
  $remoteScriptPath = "/tmp/$TempPrefix-$uploadId.sh"

  try {
    New-Item -ItemType Directory -Force -Path $localTempRoot | Out-Null
    Set-AsciiFile -Path $localRemoteScript -Content $RemoteScriptContent
    Set-AsciiFile -Path $localSftpBatch -Content ('put "{0}" "{1}"' -f $localRemoteScript, $remoteScriptPath)

    & sftp.exe -b $localSftpBatch $SshTarget
    if ($LASTEXITCODE -ne 0) {
      exit $LASTEXITCODE
    }

    & ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $SshTarget "chmod +x '$remoteScriptPath' && bash '$remoteScriptPath'"
    $remoteExit = $LASTEXITCODE
    & ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $SshTarget "rm -f '$remoteScriptPath'"
    exit $remoteExit
  } finally {
    Remove-Item -LiteralPath $localTempRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}

$remoteScript = @"
#!/usr/bin/env bash
set -euo pipefail
if [ ! -x '$MuninnExe' ]; then
  echo 'Muninn executable not found at $MuninnExe' >&2
  exit 1
fi
if [ -f '$PidPath' ] && kill -0 "`$(cat '$PidPath')" 2>/dev/null; then
  :
elif pgrep -f '[m]uninn serve .*--host nightwing' >/dev/null 2>&1; then
  :
else
  echo 'Muninn serve process is not running on Nightwing' >&2
  exit 1
fi
if [ ! -f '$StorePath' ]; then
  echo 'Muninn telemetry store is missing on Nightwing' >&2
  exit 1
fi
store_mtime="`$(stat -c %Y '$StorePath')"
now="`$(date +%s)"
store_age="`$((now - store_mtime))"
if [ "`$store_age" -gt '$MaxStoreAgeSeconds' ]; then
  echo "Muninn telemetry store is stale on Nightwing (`$store_age s old)" >&2
  exit 1
fi
identity_output="`$('$MuninnExe' move-identity-status --store '$StorePath' --host nightwing)"
if printf '%s\n' "`$identity_output" | grep -Fq 'No Muninn Move identity records found'; then
  echo 'Muninn Move identity roster is empty on Nightwing' >&2
  exit 1
fi
identity_count="`$(printf '%s\n' "`$identity_output" | grep -c 'move-identity move=')"
echo "Muninn healthy: nightwing store_age=`${store_age}s move_identities=`${identity_count}"
"@

Invoke-NightwingUploadedShell -SshTarget $SshTarget -RemoteScriptContent $remoteScript -TempPrefix "odin-nightwing-muninn-health"

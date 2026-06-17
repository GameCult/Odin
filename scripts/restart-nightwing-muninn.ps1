param(
  [string] $SshTarget = "nightwing",
  [string] $MuninnExe = "/home/metacrat/.local/bin/muninn",
  [string] $StorePath = "/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc",
  [string] $LogRoot = "/home/metacrat/.local/state/gamecult/muninn",
  [string[]] $MoveState = @(),
  [string] $MoveHost = "5C:93:A2:9C:A8:A8",
  [string] $MoveEvidenceStream = "muninn:nightwing:move-evidence",
  [int] $IntervalSeconds = 15,
  [string] $IdunnRudpHealth = "10.77.0.2:17870",
  [string] $IdunnDaemon = "nightwing-muninn",
  [string] $IdunnHealthContract = "muninn.cultnet-rudp-remote-telemetry-and-move-hid"
)

$ErrorActionPreference = "Stop"

function Quote-ShSingle([string] $Value) {
  return "'" + ($Value -replace "'", "'\''") + "'"
}

function Set-AsciiFile {
  param(
    [Parameter(Mandatory = $true)] [string] $Path,
    [Parameter(Mandatory = $true)] [string] $Content
  )

  [System.IO.File]::WriteAllText($Path, ($Content -replace "`r?`n", "`n"), [System.Text.Encoding]::ASCII)
}

function Invoke-UploadedShell {
  param(
    [Parameter(Mandatory = $true)] [string] $SshTarget,
    [Parameter(Mandatory = $true)] [string] $RemoteScriptContent
  )

  $uploadId = [guid]::NewGuid().ToString("N")
  $localTempRoot = Join-Path $env:TEMP "odin-nightwing-muninn-restart-$uploadId"
  $localRemoteScript = Join-Path $localTempRoot "restart-nightwing-muninn-$uploadId.sh"
  $localSftpBatch = Join-Path $localTempRoot "restart-nightwing-muninn-$uploadId.sftp"
  $remoteScriptPath = "/tmp/restart-nightwing-muninn-$uploadId.sh"

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

$moveStateSpecs = @($MoveState | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
if ($moveStateSpecs.Count -eq 0) {
  $claimScript = Join-Path $PSScriptRoot "claim-nightwing-usb-moves.ps1"
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $claimScript -SshTarget $SshTarget
  $sourceScript = Join-Path $PSScriptRoot "get-nightwing-move-state-sources.ps1"
  $moveStateSpecs = @(& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $sourceScript -SshTarget $SshTarget)
}

$moveStateSetLines = ($moveStateSpecs | ForEach-Object {
  "set -- ""`$@"" --move-state $(Quote-ShSingle $_)"
}) -join "`n"

$remoteScript = @"
set -eu
mkdir -p '$LogRoot'
if [ ! -x '$MuninnExe' ]; then
  echo 'Muninn executable not found at $MuninnExe' >&2
  exit 1
fi
if [ -f '$LogRoot/muninn.pid' ] && kill -0 "`$(cat '$LogRoot/muninn.pid')" 2>/dev/null; then
  kill "`$(cat '$LogRoot/muninn.pid')" 2>/dev/null || true
  sleep 1
fi
for pid in `$(pgrep -f '[m]uninn serve .*--host nightwing' 2>/dev/null || true); do
  kill "`$pid" 2>/dev/null || true
done
set -- serve \
  --store '$StorePath' \
  --log-root '$LogRoot' \
  --host nightwing \
  --move-host '$MoveHost'
$moveStateSetLines
set -- "`$@" \
  --move-evidence-stream '$MoveEvidenceStream' \
  --interval-seconds '$IntervalSeconds' \
  --idunn-rudp-health '$IdunnRudpHealth' \
  --idunn-daemon '$IdunnDaemon' \
  --idunn-health-contract '$IdunnHealthContract'
nohup '$MuninnExe' "`$@" \
  > '$LogRoot/muninn-serve.out.log' \
  2> '$LogRoot/muninn-serve.err.log' \
  < /dev/null &
echo `$! > '$LogRoot/muninn.pid'
sleep 1
kill -0 "`$(cat '$LogRoot/muninn.pid')" 2>/dev/null
if ! pgrep -af '[m]uninn serve .*--host nightwing' | grep -F -- '--idunn-rudp-health $IdunnRudpHealth' >/dev/null 2>&1; then
  echo 'Nightwing Muninn serve command line is missing Idunn RUDP health arguments' >&2
  exit 1
fi
"@

Invoke-UploadedShell -SshTarget $SshTarget -RemoteScriptContent $remoteScript

param(
  [string] $SshTarget = "nightwing",
  [string] $MuninnExe = "/home/metacrat/.local/bin/muninn",
  [string] $StorePath = "/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc",
  [string] $PidPath = "/home/metacrat/.local/state/gamecult/muninn/muninn.pid",
  [string[]] $MoveState = @(),
  [int] $IntervalSeconds = 15,
  [string] $IdunnRudpHealth = "10.77.0.2:17870"
)

$ErrorActionPreference = "Stop"

function Quote-ShSingle([string] $Value) {
  return "'" + ($Value -replace "'", "'\''") + "'"
}

$moveStateSpecs = @($MoveState | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
if ($moveStateSpecs.Count -eq 0) {
  $sourceScript = Join-Path $PSScriptRoot "get-nightwing-move-state-sources.ps1"
  $moveStateSpecs = @(& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $sourceScript -SshTarget $SshTarget)
}

$moveStateSetLines = ($moveStateSpecs | ForEach-Object {
  "set -- ""`$@"" --move-state $(Quote-ShSingle $_)"
}) -join "`n"

$remoteScript = @"
set -eu
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
set -- --health \
  --store '$StorePath' \
  --host nightwing
$moveStateSetLines
set -- "`$@" \
  --interval-seconds '$IntervalSeconds' \
  --idunn-rudp-health '$IdunnRudpHealth' \
  --idunn-daemon 'nightwing-muninn' \
  --idunn-health-contract 'muninn.cultnet-rudp-remote-telemetry-and-move-hid'
'$MuninnExe' "`$@"
"@

ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget $remoteScript
exit $LASTEXITCODE

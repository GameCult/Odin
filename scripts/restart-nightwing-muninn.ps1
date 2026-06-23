param(
  [string] $SshTarget = "nightwing",
  [string] $MuninnExe = "/home/metacrat/.local/bin/muninn",
  [string] $StorePath = "/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc",
  [string] $LogRoot = "/home/metacrat/.local/state/gamecult/muninn",
  [string[]] $MoveState = @(),
  [switch] $DiscoverMoveState,
  [switch] $ClaimUsbMoves,
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

$moveStateSpecs = @($MoveState | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
if ($ClaimUsbMoves.IsPresent -and -not $DiscoverMoveState.IsPresent -and $moveStateSpecs.Count -eq 0) {
  throw "-ClaimUsbMoves requires -DiscoverMoveState or at least one explicit -MoveState value."
}
if ($DiscoverMoveState.IsPresent) {
  $claimScript = Join-Path $PSScriptRoot "claim-nightwing-usb-moves.ps1"
  if ($ClaimUsbMoves.IsPresent) {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $claimScript -SshTarget $SshTarget
  }
  $sourceScript = Join-Path $PSScriptRoot "get-nightwing-move-state-sources.ps1"
  $moveStateSpecs = @(& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $sourceScript -SshTarget $SshTarget)
  if ($moveStateSpecs.Count -eq 0) {
    throw "Nightwing Move runtime was explicitly requested, but no live Move state sources were discovered."
  }
}

$moveStateSetLines = ($moveStateSpecs | ForEach-Object {
  "set -- ""`$@"" --move-state $(Quote-ShSingle $_)"
}) -join "`n"
$moveRuntimeSetLines = @()
if ($moveStateSpecs.Count -gt 0) {
  $moveRuntimeSetLines += $moveStateSetLines
  $moveRuntimeSetLines += "set -- ""`$@"" --move-evidence-stream $(Quote-ShSingle $MoveEvidenceStream)"
}
$moveRuntimeSetBlock = ($moveRuntimeSetLines -join "`n")

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
  --host nightwing
$moveRuntimeSetBlock
set -- "`$@" \
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

ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget $remoteScript
exit $LASTEXITCODE

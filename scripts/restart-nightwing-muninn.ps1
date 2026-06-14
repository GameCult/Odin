param(
  [string] $SshTarget = "nightwing",
  [string] $MuninnExe = "/home/metacrat/.local/bin/muninn",
  [string] $StorePath = "/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc",
  [string] $LogRoot = "/home/metacrat/.local/state/gamecult/muninn",
  [string[]] $MoveState = @(),
  [string] $MoveEvidenceStream = "muninn:nightwing:move-evidence",
  [int] $IntervalSeconds = 15
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
$moveStateSetLines
set -- "`$@" \
  --move-evidence-stream '$MoveEvidenceStream' \
  --interval-seconds '$IntervalSeconds'
nohup '$MuninnExe' "`$@" \
  > '$LogRoot/muninn-serve.out.log' \
  2> '$LogRoot/muninn-serve.err.log' \
  < /dev/null &
echo `$! > '$LogRoot/muninn.pid'
sleep 1
kill -0 "`$(cat '$LogRoot/muninn.pid')" 2>/dev/null
"@

ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget $remoteScript
exit $LASTEXITCODE

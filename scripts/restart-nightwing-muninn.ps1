param(
  [string] $SshTarget = "nightwing",
  [string] $MuninnExe = "/home/metacrat/.local/bin/muninn",
  [string] $StorePath = "/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc",
  [string] $LogRoot = "/home/metacrat/.local/state/gamecult/muninn",
  [int] $IntervalSeconds = 15
)

$ErrorActionPreference = "Stop"

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
nohup '$MuninnExe' serve \
  --store '$StorePath' \
  --log-root '$LogRoot' \
  --host nightwing \
  --interval-seconds '$IntervalSeconds' \
  > '$LogRoot/muninn-serve.out.log' \
  2> '$LogRoot/muninn-serve.err.log' \
  < /dev/null &
echo `$! > '$LogRoot/muninn.pid'
sleep 1
kill -0 "`$(cat '$LogRoot/muninn.pid')" 2>/dev/null
"@

ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget $remoteScript
exit $LASTEXITCODE

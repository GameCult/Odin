param(
  [string] $SshTarget = "nightwing",
  [string] $MuninnExe = "/home/metacrat/.local/bin/muninn",
  [string] $StorePath = "/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc",
  [string] $PidPath = "/home/metacrat/.local/state/gamecult/muninn/muninn.pid",
  [string] $MoveState = "move-usb=/dev/input/by-id/usb-Sony_Computer_Entertainment_Motion_Controller-joystick",
  [int] $IntervalSeconds = 15
)

$ErrorActionPreference = "Stop"

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
'$MuninnExe' --health \
  --store '$StorePath' \
  --host nightwing \
  --move-state '$MoveState' \
  --interval-seconds '$IntervalSeconds'
"@

ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget $remoteScript
exit $LASTEXITCODE

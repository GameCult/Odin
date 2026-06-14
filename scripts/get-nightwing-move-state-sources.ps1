param(
  [string] $SshTarget = "nightwing"
)

$ErrorActionPreference = "Stop"

$scriptPath = Join-Path $PSScriptRoot "nightwing-move-state-sources.sh"
$command = 'ssh.exe -o BatchMode=yes -o ConnectTimeout=5 ' +
  $SshTarget +
  ' "sh -s" < "' +
  $scriptPath +
  '"'
& cmd.exe /d /c $command
exit $LASTEXITCODE

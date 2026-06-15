param(
  [string] $SshTarget = "nightwing"
)

$ErrorActionPreference = "Stop"

$scriptPath = Join-Path $PSScriptRoot "nightwing-move-state-sources.sh"
$sshExe = Join-Path $env:SystemRoot "System32\OpenSSH\ssh.exe"
$command = '"' + $sshExe + '" -o BatchMode=yes -o ConnectTimeout=5 ' +
  $SshTarget +
  ' "sh -s" < "' +
  $scriptPath +
  '"'
& cmd.exe /d /c $command
exit $LASTEXITCODE

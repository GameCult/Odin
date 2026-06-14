param(
  [string] $SshTarget = "nightwing",
  [string] $HostAddress = ""
)

$ErrorActionPreference = "Stop"

$scriptPath = Join-Path $PSScriptRoot "nightwing-claim-usb-moves.sh"
$remoteArgs = ""
if (-not [string]::IsNullOrWhiteSpace($HostAddress)) {
  $remoteArgs = " " + ($HostAddress -replace "'", "'\''")
}

$command = 'ssh.exe -o BatchMode=yes -o ConnectTimeout=5 ' +
  $SshTarget +
  ' "sh -s' +
  $remoteArgs +
  '" < "' +
  $scriptPath +
  '"'

cmd.exe /c $command
exit $LASTEXITCODE

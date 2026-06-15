param(
  [string] $SshTarget = "nightwing",
  [string] $HostAddress = ""
)

$ErrorActionPreference = "Stop"

$scriptPath = Join-Path $PSScriptRoot "nightwing-claim-usb-moves.sh"
$sshExe = Join-Path $env:SystemRoot "System32\OpenSSH\ssh.exe"
$remoteArgs = ""
if (-not [string]::IsNullOrWhiteSpace($HostAddress)) {
  $remoteArgs = " " + ($HostAddress -replace "'", "'\''")
}

$command = '"' + $sshExe + '" -o BatchMode=yes -o ConnectTimeout=5 ' +
  $SshTarget +
  ' "sh -s' +
  $remoteArgs +
  '" < "' +
  $scriptPath +
  '"'

cmd.exe /c $command
exit $LASTEXITCODE

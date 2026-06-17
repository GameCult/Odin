param(
  [string] $SshTarget = "nightwing",
  [string] $HostAddress = ""
)

$ErrorActionPreference = "Stop"

$scriptPath = Join-Path $PSScriptRoot "nightwing-claim-usb-moves.sh"
$sshExe = Join-Path $env:SystemRoot "System32\OpenSSH\ssh.exe"

function Quote-ShSingle([string] $Value) {
  return "'" + ($Value -replace "'", "'\''") + "'"
}

$scriptContent = [System.IO.File]::ReadAllText($scriptPath) -replace "`r?`n", "`n"
$script64 = [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($scriptContent))
$remoteArgs = ""
if (-not [string]::IsNullOrWhiteSpace($HostAddress)) {
  $remoteArgs = " " + (Quote-ShSingle $HostAddress)
}

$command = '"' + $sshExe + '" -o BatchMode=yes -o ConnectTimeout=5 ' +
  $SshTarget +
  ' "printf %s ' +
  (Quote-ShSingle $script64) +
  ' | base64 -d | sh -s --' +
  $remoteArgs +
  '"'

cmd.exe /c $command
exit $LASTEXITCODE

param(
  [string] $SshTarget = "nightwing"
)

$ErrorActionPreference = "Stop"

$scriptPath = Join-Path $PSScriptRoot "nightwing-move-state-sources.sh"
$sshExe = Join-Path $env:SystemRoot "System32\OpenSSH\ssh.exe"

function Quote-ShSingle([string] $Value) {
  return "'" + ($Value -replace "'", "'\''") + "'"
}

$scriptContent = [System.IO.File]::ReadAllText($scriptPath) -replace "`r?`n", "`n"
$script64 = [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($scriptContent))
$command = '"' + $sshExe + '" -o BatchMode=yes -o ConnectTimeout=5 ' +
  $SshTarget +
  ' "printf %s ' +
  (Quote-ShSingle $script64) +
  ' | base64 -d | sh -s"'
& cmd.exe /d /c $command
exit $LASTEXITCODE

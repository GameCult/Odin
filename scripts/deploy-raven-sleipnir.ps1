param(
  [string] $LocalSleipnirExe = "E:\Projects\Odin\target\release\sleipnir.exe",
  [string] $RavenHost = "raven",
  [string] $RemoteSleipnirExe = "C:\Meta\Odin\Sleipnir\sleipnir.exe",
  [string] $SshUser = "madman's lullaby",
  [string] $IdentityFile = "C:\Users\Meta\.ssh\id_ed25519_192_168_1_84",
  [int] $ConnectTimeoutSeconds = 10
)

$ErrorActionPreference = "Stop"

if ($env:IDUNN_ACTUATOR -ne "1" -or $env:IDUNN_COMMAND_AUTHORITY -ne "idunn-daemon") {
  throw "deploy-raven-sleipnir.ps1 is an Idunn actuator body. Deploy by poking Idunn; direct binary copy is not an owned path."
}
if (-not (Test-Path -LiteralPath $LocalSleipnirExe)) {
  throw "Local Sleipnir release binary not found at $LocalSleipnirExe"
}

function ConvertTo-PowerShellStringLiteral {
  param([Parameter(Mandatory = $true)] [string] $Value)
  return "'" + $Value.Replace("'", "''") + "'"
}

function Get-SshTarget {
  if ([string]::IsNullOrWhiteSpace($SshUser)) {
    return $RavenHost
  }
  return "${SshUser}@${RavenHost}"
}

$sshArgs = @(
  "-o", "BatchMode=yes",
  "-o", "ConnectTimeout=$ConnectTimeoutSeconds",
  "-o", "ConnectionAttempts=1"
)
if (-not [string]::IsNullOrWhiteSpace($IdentityFile)) {
  $sshArgs += @("-i", $IdentityFile)
}

$target = Get-SshTarget
$remoteNew = "$RemoteSleipnirExe.new"
$remoteDir = Split-Path -Parent $RemoteSleipnirExe
$remoteNewSftp = $remoteNew.Replace("\", "/")
$prepare = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"
New-Item -ItemType Directory -Force -Path $(ConvertTo-PowerShellStringLiteral $remoteDir) | Out-Null
Get-CimInstance Win32_Process |
  Where-Object { `$_.Name -ieq "sleipnir.exe" } |
  ForEach-Object { taskkill.exe /PID `$_.ProcessId /T /F | Out-Null }
Remove-Item -LiteralPath $(ConvertTo-PowerShellStringLiteral $remoteNew) -Force -ErrorAction SilentlyContinue
exit 0
"@
$encodedPrepare = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($prepare))
& ssh.exe @sshArgs $target "powershell.exe -NoProfile -ExecutionPolicy Bypass -EncodedCommand $encodedPrepare"
if ($LASTEXITCODE -ne 0) {
  exit $LASTEXITCODE
}

$sftpBatch = New-TemporaryFile
try {
  $sftpLines = @(
    "put `"$LocalSleipnirExe`" `"$remoteNewSftp`""
  )
  Set-Content -LiteralPath $sftpBatch.FullName -Value $sftpLines -Encoding ASCII
  & sftp.exe @sshArgs -b $sftpBatch.FullName $target
  if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
  }
} finally {
  Remove-Item -LiteralPath $sftpBatch.FullName -Force -ErrorAction SilentlyContinue
}

$install = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"
if (Test-Path -LiteralPath $(ConvertTo-PowerShellStringLiteral $RemoteSleipnirExe)) {
  Remove-Item -LiteralPath $(ConvertTo-PowerShellStringLiteral $RemoteSleipnirExe) -Force
}
Move-Item -LiteralPath $(ConvertTo-PowerShellStringLiteral $remoteNew) -Destination $(ConvertTo-PowerShellStringLiteral $RemoteSleipnirExe) -Force
exit 0
"@
$encodedInstall = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($install))
& ssh.exe @sshArgs $target "powershell.exe -NoProfile -ExecutionPolicy Bypass -EncodedCommand $encodedInstall"
if ($LASTEXITCODE -ne 0) {
  exit $LASTEXITCODE
}

& "$PSScriptRoot\restart-raven-sleipnir.ps1" -RavenHost $RavenHost -SleipnirExe $RemoteSleipnirExe -SshUser $SshUser -IdentityFile $IdentityFile -ConnectTimeoutSeconds $ConnectTimeoutSeconds
if ($LASTEXITCODE -ne 0) {
  exit $LASTEXITCODE
}

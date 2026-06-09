param(
  [Parameter(Mandatory = $true)] [string] $AppId,
  [Parameter(Mandatory = $true)] [string] $RepoRoot,
  [Parameter(Mandatory = $true)] [string] $RemoteAppHome,
  [Parameter(Mandatory = $true)] [string] $CheckScript,
  [string] $SshTarget = "yggdrasil"
)

$ErrorActionPreference = "Stop"

$localCommit = (git -C $RepoRoot rev-parse HEAD).Trim()
if ([string]::IsNullOrWhiteSpace($localCommit)) {
  throw "Could not determine git revision for $RepoRoot"
}

$remoteCheck = "/home/gamecultadmin/$([IO.Path]::GetFileName($CheckScript))"
$scpArgs = @("-o", "BatchMode=yes", "-o", "ConnectTimeout=10")
$sshArgs = @("-o", "BatchMode=yes", "-o", "ConnectTimeout=10", $SshTarget)
scp.exe @scpArgs $CheckScript "${SshTarget}:$remoteCheck" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "scp check script failed for $AppId" }

$manifest = ssh.exe @sshArgs "cat '$RemoteAppHome/deployment-manifest.txt' 2>/dev/null"
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($manifest)) {
  throw "$AppId deploy manifest is missing on Yggdrasil."
}

$remoteCommit = ""
foreach ($line in $manifest) {
  if ($line -like "gitCommit=*") {
    $remoteCommit = $line.Substring("gitCommit=".Length).Trim()
  }
}

if ($remoteCommit -ne $localCommit) {
  throw "$AppId deploy is stale. remote=$remoteCommit local=$localCommit"
}

ssh.exe @sshArgs "chmod +x '$remoteCheck' && sudo -n bash '$remoteCheck'"
if ($LASTEXITCODE -ne 0) {
  throw "$AppId remote health check failed."
}

Write-Host "$AppId is active and deployed at $localCommit."

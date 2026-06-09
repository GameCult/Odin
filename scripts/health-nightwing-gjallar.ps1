param(
  [string] $GjallarRoot = "E:\Projects\Gjallar",
  [string] $SshTarget = "nightwing",
  [string] $RemoteManifest = "/opt/gamecult/gjallar/gamecult-gjallar-deploy-manifest.txt"
)

$ErrorActionPreference = "Stop"

ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "systemctl is-active --quiet gjallar.service"
if ($LASTEXITCODE -ne 0) {
  throw "gjallar.service is not active on Nightwing."
}

$localCommit = (git -C $GjallarRoot rev-parse HEAD).Trim()
if ([string]::IsNullOrWhiteSpace($localCommit)) {
  throw "Could not determine local Gjallar git revision."
}

$manifest = ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "cat '$RemoteManifest' 2>/dev/null"
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($manifest)) {
  throw "Nightwing Gjallar deploy manifest is missing."
}

$remoteCommit = ""
foreach ($line in $manifest) {
  if ($line -like "gitCommit=*") {
    $remoteCommit = $line.Substring("gitCommit=".Length).Trim()
  }
}

if ($remoteCommit -ne $localCommit) {
  throw "Nightwing Gjallar deploy is stale. remote=$remoteCommit local=$localCommit"
}

Write-Host "Nightwing Gjallar is active and deployed at $localCommit."

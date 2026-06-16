param(
  [Parameter(Mandatory = $true)] [string] $AppId,
  [Parameter(Mandatory = $true)] [string] $RepoRoot,
  [Parameter(Mandatory = $true)] [string] $AppUser,
  [Parameter(Mandatory = $true)] [string] $RemoteAppHome,
  [Parameter(Mandatory = $true)] [string] $RemoteTarballName,
  [Parameter(Mandatory = $true)] [string] $DeployScript,
  [Parameter(Mandatory = $true)] [string] $CheckScript,
  [string] $SshTarget = "yggdrasil"
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $RepoRoot)) {
  throw "Repo root not found: $RepoRoot"
}
if (-not (Test-Path -LiteralPath $DeployScript)) {
  throw "Deploy script not found: $DeployScript"
}
if (-not (Test-Path -LiteralPath $CheckScript)) {
  throw "Check script not found: $CheckScript"
}

$commit = (git -C $RepoRoot rev-parse HEAD).Trim()
if ([string]::IsNullOrWhiteSpace($commit)) {
  throw "Could not determine git revision for $RepoRoot"
}

$scratch = Join-Path "E:\Projects\Odin\scratch\idunn-deploy" $AppId
$tarPath = Join-Path $scratch $RemoteTarballName
$manifestPath = Join-Path $scratch "deployment-manifest.txt"
New-Item -ItemType Directory -Force -Path $scratch | Out-Null

git -C $RepoRoot archive --format=tar --output=$tarPath HEAD
if ($LASTEXITCODE -ne 0) {
  throw "git archive failed for $RepoRoot"
}

$hash = (Get-FileHash $tarPath -Algorithm SHA256).Hash.ToLowerInvariant()
$deployedAt = [DateTimeOffset]::UtcNow.ToString("O")
@(
  "schema=gamecult.idunn.deployment_manifest.v1"
  "appId=$AppId"
  "repoRoot=$RepoRoot"
  "gitCommit=$commit"
  "artifact=$RemoteTarballName"
  "sha256=$hash"
  "deployedAtUtc=$deployedAt"
) | Set-Content -Encoding ASCII -LiteralPath $manifestPath

$remoteDeploy = "/home/gamecultadmin/$([IO.Path]::GetFileName($DeployScript))"
$remoteCheck = "/home/gamecultadmin/$([IO.Path]::GetFileName($CheckScript))"
$remoteTarball = "/home/gamecultadmin/$RemoteTarballName"
$remoteManifest = "/home/gamecultadmin/$AppId-deployment-manifest.txt"
$remoteRunner = "/home/gamecultadmin/run-$AppId-deploy.sh"
$targetTarball = "$RemoteAppHome/$RemoteTarballName"
$targetManifest = "$RemoteAppHome/deployment-manifest.txt"
$sshArgs = @("-o", "BatchMode=yes", "-o", "ConnectTimeout=10", $SshTarget)
$scpArgs = @("-o", "BatchMode=yes", "-o", "ConnectTimeout=10")
$runnerPath = Join-Path $scratch "run-$AppId-deploy.sh"

$runnerScript = @"
#!/usr/bin/env bash
set -euo pipefail

sudo -n install -d -o '$AppUser' -g '$AppUser' '$RemoteAppHome'
sudo -n install -o '$AppUser' -g '$AppUser' -m 600 '$remoteTarball' '$targetTarball'
sudo -n install -m 644 '$remoteManifest' '$targetManifest'
chmod +x '$remoteDeploy' '$remoteCheck'
sudo -n bash '$remoteDeploy'

check_ok=0
for i in `$(seq 1 30); do
  if sudo -n bash '$remoteCheck'; then
    check_ok=1
    break
  fi
  sleep 2
done

if [ "`$check_ok" -ne 1 ]; then
  sudo -n bash '$remoteCheck'
  exit 1
fi
"@.Replace("`r`n", "`n")
[System.IO.File]::WriteAllText($runnerPath, $runnerScript, [System.Text.Encoding]::ASCII)

scp.exe @scpArgs $tarPath "${SshTarget}:$remoteTarball"
if ($LASTEXITCODE -ne 0) { throw "scp source tarball failed for $AppId" }
scp.exe @scpArgs $DeployScript "${SshTarget}:$remoteDeploy"
if ($LASTEXITCODE -ne 0) { throw "scp deploy script failed for $AppId" }
scp.exe @scpArgs $CheckScript "${SshTarget}:$remoteCheck"
if ($LASTEXITCODE -ne 0) { throw "scp check script failed for $AppId" }
scp.exe @scpArgs $manifestPath "${SshTarget}:$remoteManifest"
if ($LASTEXITCODE -ne 0) { throw "scp manifest failed for $AppId" }
scp.exe @scpArgs $runnerPath "${SshTarget}:$remoteRunner"
if ($LASTEXITCODE -ne 0) { throw "scp runner script failed for $AppId" }

ssh.exe @sshArgs "chmod +x '$remoteRunner' && bash '$remoteRunner'"
$remoteExit = $LASTEXITCODE
ssh.exe @sshArgs "rm -f '$remoteRunner'"
if ($remoteExit -ne 0) {
  throw "remote deploy/check failed for $AppId"
}

Remove-Item -LiteralPath $runnerPath -Force -ErrorAction SilentlyContinue

Write-Host "$AppId deployed to Yggdrasil at $commit."
Write-Host "  artifactSha256=$hash"

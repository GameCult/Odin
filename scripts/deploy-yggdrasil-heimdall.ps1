param(
  [string] $SshTarget = "yggdrasil",
  [string] $RepoRoot = "E:\Projects\Heimdall",
  [string] $CultLibRoot = "E:\Projects\CultLib",
  [string] $AppUser = "heimdall",
  [string] $RemoteAppHome = "/srv/heimdall",
  [string] $RemoteTarballName = "heimdall-source.tar",
  [string] $RemoteCultLibTarballName = "heimdall-cultlib.tar",
  [string] $DeployScript = "E:\Projects\gamecult-ops\scripts\deploy-heimdall.sh",
  [string] $CheckScript = "E:\Projects\gamecult-ops\scripts\check-heimdall.sh",
  [string] $UpstreamRemote = "origin",
  [string] $UpstreamBranch = "main",
  [string] $CultLibUpstreamRemote = "origin",
  [string] $CultLibUpstreamBranch = "main"
)

$ErrorActionPreference = "Stop"

foreach ($path in @($RepoRoot, $CultLibRoot, $DeployScript, $CheckScript)) {
  if (-not (Test-Path -LiteralPath $path)) {
    throw "Required path not found: $path"
  }
}

$sourceRef = "$UpstreamRemote/$UpstreamBranch"
$cultLibSourceRef = "$CultLibUpstreamRemote/$CultLibUpstreamBranch"
git -C $RepoRoot fetch --prune $UpstreamRemote $UpstreamBranch
if ($LASTEXITCODE -ne 0) { throw "git fetch failed for $RepoRoot $sourceRef" }
git -C $CultLibRoot fetch --prune $CultLibUpstreamRemote $CultLibUpstreamBranch
if ($LASTEXITCODE -ne 0) { throw "git fetch failed for $CultLibRoot $cultLibSourceRef" }

$heimdallCommit = (git -C $RepoRoot rev-parse $sourceRef).Trim()
if ([string]::IsNullOrWhiteSpace($heimdallCommit)) {
  throw "Could not determine Heimdall git revision."
}
$cultLibCommit = (git -C $CultLibRoot rev-parse $cultLibSourceRef).Trim()
if ([string]::IsNullOrWhiteSpace($cultLibCommit)) {
  throw "Could not determine CultLib git revision."
}

$scratch = Join-Path "E:\Projects\Odin\scratch\idunn-deploy" "yggdrasil-heimdall"
$sourceTarPath = Join-Path $scratch $RemoteTarballName
$cultLibTarPath = Join-Path $scratch $RemoteCultLibTarballName
$manifestPath = Join-Path $scratch "deployment-manifest.txt"
New-Item -ItemType Directory -Force -Path $scratch | Out-Null

git -C $RepoRoot archive --format=tar --output=$sourceTarPath $sourceRef
if ($LASTEXITCODE -ne 0) {
  throw "git archive failed for $RepoRoot $sourceRef"
}

if (Test-Path -LiteralPath $cultLibTarPath) {
  Remove-Item -LiteralPath $cultLibTarPath -Force
}
git -C $CultLibRoot archive --format=tar --output=$cultLibTarPath $cultLibSourceRef packages/cultnet-ts packages/cultcache-ts
if ($LASTEXITCODE -ne 0) {
  throw "CultLib tar build failed for $CultLibRoot $cultLibSourceRef"
}

$sourceHash = (Get-FileHash $sourceTarPath -Algorithm SHA256).Hash.ToLowerInvariant()
$cultLibHash = (Get-FileHash $cultLibTarPath -Algorithm SHA256).Hash.ToLowerInvariant()
$deployedAt = [DateTimeOffset]::UtcNow.ToString("O")
@(
  "schema=gamecult.idunn.deployment_manifest.v1"
  "appId=yggdrasil-heimdall"
  "repoRoot=$RepoRoot"
  "upstreamRemote=$UpstreamRemote"
  "upstreamBranch=$UpstreamBranch"
  "sourceRef=$sourceRef"
  "gitCommit=$heimdallCommit"
  "artifact=$RemoteTarballName"
  "sha256=$sourceHash"
  "cultLibRepoRoot=$CultLibRoot"
  "cultLibUpstreamRemote=$CultLibUpstreamRemote"
  "cultLibUpstreamBranch=$CultLibUpstreamBranch"
  "cultLibSourceRef=$cultLibSourceRef"
  "cultLibGitCommit=$cultLibCommit"
  "cultLibArtifact=$RemoteCultLibTarballName"
  "cultLibSha256=$cultLibHash"
  "deployedAtUtc=$deployedAt"
) | Set-Content -Encoding ASCII -LiteralPath $manifestPath

$remoteDeploy = "/home/gamecultadmin/$([IO.Path]::GetFileName($DeployScript))"
$remoteCheck = "/home/gamecultadmin/$([IO.Path]::GetFileName($CheckScript))"
$remoteSourceTarball = "/home/gamecultadmin/$RemoteTarballName"
$remoteCultLibTarball = "/home/gamecultadmin/$RemoteCultLibTarballName"
$remoteManifest = "/home/gamecultadmin/yggdrasil-heimdall-deployment-manifest.txt"
$remoteRunner = "/home/gamecultadmin/run-yggdrasil-heimdall-deploy.sh"
$targetSourceTarball = "$RemoteAppHome/$RemoteTarballName"
$targetCultLibTarball = "$RemoteAppHome/$RemoteCultLibTarballName"
$targetManifest = "$RemoteAppHome/deployment-manifest.txt"

$sftpBatchPath = Join-Path $scratch "yggdrasil-heimdall-upload.sftp"
$runnerPath = Join-Path $scratch "run-yggdrasil-heimdall-deploy.sh"
$runnerScript = @"
#!/usr/bin/env bash
set -euo pipefail

sudo -n install -d -o '$AppUser' -g '$AppUser' '$RemoteAppHome'
sudo -n install -o '$AppUser' -g '$AppUser' -m 600 '$remoteSourceTarball' '$targetSourceTarball'
sudo -n install -o '$AppUser' -g '$AppUser' -m 600 '$remoteCultLibTarball' '$targetCultLibTarball'
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
@(
  "put ""$sourceTarPath"" ""$remoteSourceTarball"""
  "put ""$cultLibTarPath"" ""$remoteCultLibTarball"""
  "put ""$DeployScript"" ""$remoteDeploy"""
  "put ""$CheckScript"" ""$remoteCheck"""
  "put ""$manifestPath"" ""$remoteManifest"""
  "put ""$runnerPath"" ""$remoteRunner"""
) | Set-Content -Encoding ASCII -LiteralPath $sftpBatchPath

try {
  & sftp.exe -b $sftpBatchPath $SshTarget
  if ($LASTEXITCODE -ne 0) {
    throw "sftp upload failed for yggdrasil-heimdall"
  }

  & ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $SshTarget "chmod +x '$remoteRunner' && bash '$remoteRunner'"
  $remoteExit = $LASTEXITCODE
  & ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $SshTarget "rm -f '$remoteRunner'"
  if ($remoteExit -ne 0) {
    throw "remote deploy/check failed for yggdrasil-heimdall"
  }
}
finally {
  Remove-Item -LiteralPath $runnerPath -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $sftpBatchPath -Force -ErrorAction SilentlyContinue
}

Write-Host "yggdrasil-heimdall deployed with CultLib snapshot."
Write-Host "  heimdallCommit=$heimdallCommit"
Write-Host "  cultLibCommit=$cultLibCommit"
Write-Host "  artifactSha256=$sourceHash"
Write-Host "  cultLibSha256=$cultLibHash"

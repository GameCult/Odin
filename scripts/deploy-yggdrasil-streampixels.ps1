param(
  [string] $SshTarget = "yggdrasil",
  [string] $RepoRoot = "E:\Projects\StreamPixels",
  [string] $CultLibRoot = "E:\Projects\CultLib",
  [string] $AppUser = "streampixels",
  [string] $RemoteAppHome = "/srv/streampixels",
  [string] $RemoteTarballName = "streampixels-source.tar",
  [string] $RemoteCultLibTarballName = "streampixels-cultlib.tar",
  [string] $DeployScript = "E:\Projects\gamecult-ops\scripts\deploy-streampixels-preview.sh",
  [string] $CheckScript = "E:\Projects\gamecult-ops\scripts\check-streampixels-preview.sh"
)

$ErrorActionPreference = "Stop"

foreach ($path in @($RepoRoot, $CultLibRoot, $DeployScript, $CheckScript)) {
  if (-not (Test-Path -LiteralPath $path)) {
    throw "Required path not found: $path"
  }
}

$streamPixelsCommit = (git -C $RepoRoot rev-parse HEAD).Trim()
if ([string]::IsNullOrWhiteSpace($streamPixelsCommit)) {
  throw "Could not determine StreamPixels git revision."
}
$cultLibCommit = (git -C $CultLibRoot rev-parse HEAD).Trim()
if ([string]::IsNullOrWhiteSpace($cultLibCommit)) {
  throw "Could not determine CultLib git revision."
}

$scratch = Join-Path "E:\Projects\Odin\scratch\idunn-deploy" "yggdrasil-streampixels"
$sourceTarPath = Join-Path $scratch $RemoteTarballName
$cultLibTarPath = Join-Path $scratch $RemoteCultLibTarballName
$manifestPath = Join-Path $scratch "deployment-manifest.txt"
New-Item -ItemType Directory -Force -Path $scratch | Out-Null

git -C $RepoRoot archive --format=tar --output=$sourceTarPath HEAD
if ($LASTEXITCODE -ne 0) {
  throw "git archive failed for $RepoRoot"
}

if (Test-Path -LiteralPath $cultLibTarPath) {
  Remove-Item -LiteralPath $cultLibTarPath -Force
}
tar.exe -cf $cultLibTarPath -C $CultLibRoot packages/cultnet-ts packages/cultcache-ts
if ($LASTEXITCODE -ne 0) {
  throw "CultLib tar build failed for $CultLibRoot"
}

$sourceHash = (Get-FileHash $sourceTarPath -Algorithm SHA256).Hash.ToLowerInvariant()
$cultLibHash = (Get-FileHash $cultLibTarPath -Algorithm SHA256).Hash.ToLowerInvariant()
$deployedAt = [DateTimeOffset]::UtcNow.ToString("O")
@(
  "schema=gamecult.idunn.deployment_manifest.v1"
  "appId=yggdrasil-streampixels"
  "repoRoot=$RepoRoot"
  "gitCommit=$streamPixelsCommit"
  "artifact=$RemoteTarballName"
  "sha256=$sourceHash"
  "cultLibRepoRoot=$CultLibRoot"
  "cultLibGitCommit=$cultLibCommit"
  "cultLibArtifact=$RemoteCultLibTarballName"
  "cultLibSha256=$cultLibHash"
  "deployedAtUtc=$deployedAt"
) | Set-Content -Encoding ASCII -LiteralPath $manifestPath

$remoteDeploy = "/home/gamecultadmin/$([IO.Path]::GetFileName($DeployScript))"
$remoteCheck = "/home/gamecultadmin/$([IO.Path]::GetFileName($CheckScript))"
$remoteSourceTarball = "/home/gamecultadmin/$RemoteTarballName"
$remoteCultLibTarball = "/home/gamecultadmin/$RemoteCultLibTarballName"
$remoteManifest = "/home/gamecultadmin/yggdrasil-streampixels-deployment-manifest.txt"
$remoteRunner = "/home/gamecultadmin/run-yggdrasil-streampixels-deploy.sh"
$targetSourceTarball = "$RemoteAppHome/$RemoteTarballName"
$targetCultLibTarball = "$RemoteAppHome/$RemoteCultLibTarballName"
$targetManifest = "$RemoteAppHome/deployment-manifest.txt"

$sftpBatchPath = Join-Path $scratch "yggdrasil-streampixels-upload.sftp"
$runnerPath = Join-Path $scratch "run-yggdrasil-streampixels-deploy.sh"
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
    throw "sftp upload failed for yggdrasil-streampixels"
  }

  & ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $SshTarget "chmod +x '$remoteRunner' && bash '$remoteRunner'"
  $remoteExit = $LASTEXITCODE
  & ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $SshTarget "rm -f '$remoteRunner'"
  if ($remoteExit -ne 0) {
    throw "remote deploy/check failed for yggdrasil-streampixels"
  }
}
finally {
  Remove-Item -LiteralPath $runnerPath -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $sftpBatchPath -Force -ErrorAction SilentlyContinue
}

Write-Host "yggdrasil-streampixels deployed with CultLib snapshot."
Write-Host "  streamPixelsCommit=$streamPixelsCommit"
Write-Host "  cultLibCommit=$cultLibCommit"
Write-Host "  artifactSha256=$sourceHash"
Write-Host "  cultLibSha256=$cultLibHash"

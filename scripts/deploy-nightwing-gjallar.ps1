param(
  [string] $GjallarRoot = "E:\Projects\Gjallar",
  [string] $SshTarget = "nwroot",
  [string] $RemoteDir = "/opt/gamecult/gjallar",
  [string] $CultCachePath = "/var/lib/gamecult/gjallar/cultcache/gjallar.service.cc",
  [string] $IdunnRudpHealth = "10.77.0.2:17870",
  [string] $IdunnDaemon = "nightwing-gjallar",
  [string] $IdunnHealthContract = "gjallar.cultnet-rudp-framebuffer-composition-health",
  [string] $UpstreamRemote = "origin",
  [string] $UpstreamBranch = "main"
)

$ErrorActionPreference = "Stop"

$sourceRef = "$UpstreamRemote/$UpstreamBranch"
$scratchRoot = Join-Path $GjallarRoot "scratch"
$releaseSource = Join-Path $scratchRoot "idunn-release-source"
$releaseTarPath = Join-Path $scratchRoot "gjallar-source-release.tar"
$publishDir = Join-Path $scratchRoot "publish\gjallar"
$tarPath = Join-Path $GjallarRoot "scratch\gjallar-publish.tar"
$projectPath = Join-Path $releaseSource "src\Gjallar\Gjallar.csproj"
$manifestPath = Join-Path $publishDir "gamecult-gjallar-deploy-manifest.txt"

git -C $GjallarRoot fetch --prune $UpstreamRemote $UpstreamBranch
if ($LASTEXITCODE -ne 0) {
  throw "git fetch failed for $GjallarRoot $sourceRef"
}

$commit = (git -C $GjallarRoot rev-parse $sourceRef).Trim()
if ([string]::IsNullOrWhiteSpace($commit)) {
  throw "Could not determine Gjallar git revision."
}

New-Item -ItemType Directory -Force -Path $scratchRoot | Out-Null
if (Test-Path -LiteralPath $releaseSource) {
  $resolvedScratch = (Resolve-Path -LiteralPath $scratchRoot).Path
  $resolvedRelease = (Resolve-Path -LiteralPath $releaseSource).Path
  if (-not $resolvedRelease.StartsWith($resolvedScratch, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to remove release source outside scratch: $resolvedRelease"
  }
  Remove-Item -LiteralPath $releaseSource -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $releaseSource | Out-Null
git -C $GjallarRoot archive --format=tar --output=$releaseTarPath $sourceRef
if ($LASTEXITCODE -ne 0) {
  throw "git archive failed for $GjallarRoot $sourceRef"
}
tar -xf $releaseTarPath -C $releaseSource
if ($LASTEXITCODE -ne 0) {
  throw "source extraction failed for $releaseTarPath"
}

dotnet publish $projectPath -c Release -r linux-x64 --self-contained true -o $publishDir
if ($LASTEXITCODE -ne 0) {
  throw "dotnet publish failed with exit code $LASTEXITCODE"
}

$dllPath = Join-Path $publishDir "Gjallar.dll"
$hash = (Get-FileHash $dllPath -Algorithm SHA256).Hash.ToLowerInvariant()
$deployedAt = [DateTimeOffset]::UtcNow.ToString("O")
@(
  "schema=gamecult.gjallar.deployment_manifest.v1"
  "upstreamRemote=$UpstreamRemote"
  "upstreamBranch=$UpstreamBranch"
  "sourceRef=$sourceRef"
  "gitCommit=$commit"
  "artifact=Gjallar.dll"
  "sha256=$hash"
  "deployedAtUtc=$deployedAt"
) | Set-Content -Encoding ASCII -LiteralPath $manifestPath

tar -cf $tarPath -C $publishDir .
if ($LASTEXITCODE -ne 0) {
  throw "tar failed with exit code $LASTEXITCODE"
}

scp.exe $tarPath "${SshTarget}:/tmp/gjallar-publish.tar"
if ($LASTEXITCODE -ne 0) {
  throw "scp failed with exit code $LASTEXITCODE"
}

$cultCacheDir = if ($CultCachePath.LastIndexOf('/') -gt 0) { $CultCachePath.Substring(0, $CultCachePath.LastIndexOf('/')) } else { "." }
$dropIn = @"
[Service]
Environment=GJALLAR_CULTCACHE_PATH=$CultCachePath
Environment=GJALLAR_IDUNN_RUDP_HEALTH=$IdunnRudpHealth
Environment=GJALLAR_IDUNN_DAEMON=$IdunnDaemon
Environment=GJALLAR_IDUNN_HEALTH_CONTRACT=$IdunnHealthContract
"@
$dropInBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($dropIn))

ssh.exe $SshTarget "mkdir -p '$RemoteDir' '$cultCacheDir' /etc/systemd/system/gjallar.service.d && tar -xf /tmp/gjallar-publish.tar -C '$RemoteDir' && chmod +x '$RemoteDir/Gjallar' && printf '%s' '$dropInBase64' | base64 -d > /etc/systemd/system/gjallar.service.d/idunn-rudp-health.conf && systemctl daemon-reload && systemctl restart gjallar.service && systemctl is-active --quiet gjallar.service"
if ($LASTEXITCODE -ne 0) {
  throw "remote deploy or restart failed with exit code $LASTEXITCODE"
}

Write-Host "Gjallar deployed to Nightwing."
Write-Host "  gitCommit=$commit"
Write-Host "  sha256=$hash"

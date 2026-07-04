param(
  [string] $GjallarRoot = "E:\Projects\Gjallar",
  [string] $CultLibRoot = "E:\Projects\CultLib",
  [string] $CultMathRoot = "E:\Projects\CultMath",
  [string] $SshTarget = "nwroot",
  [string] $RemoteDir = "/opt/gamecult/gjallar",
  [string] $CultCachePath = "/var/lib/gamecult/gjallar/cultcache/gjallar.service.cc",
  [string] $OdinCultMeshUri = $(if ($env:GJALLAR_ODIN_CULTMESH_URI) { $env:GJALLAR_ODIN_CULTMESH_URI } elseif ($env:ODIN_CULTMESH_URI) { $env:ODIN_CULTMESH_URI } else { "cultmesh://odin/rendezvous/provider-catalog" }),
  [string] $IdunnRudpHealth = $(if ($env:GJALLAR_IDUNN_RUDP_HEALTH) { $env:GJALLAR_IDUNN_RUDP_HEALTH } else { $env:IDUNN_RUDP_HEALTH }),
  [string] $IdunnDaemon = "nightwing-gjallar",
  [string] $IdunnHealthContract = "gjallar.cultnet-rudp-framebuffer-composition-health",
  [string] $UpstreamRemote = "origin",
  [string] $UpstreamBranch = "main"
)

$ErrorActionPreference = "Stop"

function Remove-ScratchPath {
  param(
    [string] $ScratchRoot,
    [string] $TargetPath
  )

  if (-not (Test-Path -LiteralPath $TargetPath)) {
    return
  }

  $resolvedScratch = (Resolve-Path -LiteralPath $ScratchRoot).Path
  $resolvedTarget = (Resolve-Path -LiteralPath $TargetPath).Path
  if (-not $resolvedTarget.StartsWith($resolvedScratch, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to remove scratch path outside scratch root: $resolvedTarget"
  }

  Remove-Item -LiteralPath $TargetPath -Recurse -Force
}

function Export-GitArchive {
  param(
    [string] $RepoRoot,
    [string] $Ref,
    [string] $Destination,
    [string] $TarPath
  )

  Remove-ScratchPath -ScratchRoot (Split-Path -Parent $Destination) -TargetPath $Destination
  New-Item -ItemType Directory -Force -Path $Destination | Out-Null
  git -C $RepoRoot archive --format=tar --output=$TarPath $Ref
  if ($LASTEXITCODE -ne 0) {
    throw "git archive failed for $RepoRoot $Ref"
  }
  tar -xf $TarPath -C $Destination
  if ($LASTEXITCODE -ne 0) {
    throw "source extraction failed for $TarPath"
  }
}

if ($env:IDUNN_ACTUATOR -ne "1" -or $env:IDUNN_COMMAND_AUTHORITY -ne "idunn-daemon") {
  throw "This deployment script is an Idunn actuator. Agents must configure Idunn release targets and let Idunn run deployment; do not invoke deploy scripts manually."
}
if ($env:GJALLAR_ODIN_CULTNET_RUDP -or $env:ODIN_CULTNET_RUDP) {
  throw "GJALLAR_ODIN_CULTNET_RUDP and ODIN_CULTNET_RUDP were removed. Configure GJALLAR_ODIN_CULTMESH_URI or ODIN_CULTMESH_URI with a cultmesh:// Odin route."
}
if ([string]::IsNullOrWhiteSpace($OdinCultMeshUri) -or -not $OdinCultMeshUri.StartsWith("cultmesh://", [StringComparison]::OrdinalIgnoreCase)) {
  throw "Odin CultMesh URI must be supplied by GJALLAR_ODIN_CULTMESH_URI, ODIN_CULTMESH_URI, or -OdinCultMeshUri and must start with cultmesh://."
}
if ([string]::IsNullOrWhiteSpace($IdunnRudpHealth)) {
  throw "Idunn RUDP health endpoint must be supplied by GJALLAR_IDUNN_RUDP_HEALTH, IDUNN_RUDP_HEALTH, or -IdunnRudpHealth; no WireGuard endpoint default is allowed."
}

$sourceRef = "$UpstreamRemote/$UpstreamBranch"
$scratchRoot = Join-Path $GjallarRoot "scratch"
$releaseSource = Join-Path $scratchRoot "idunn-release-source"
$cultLibRelease = Join-Path $scratchRoot "CultLib"
$cultMathRelease = Join-Path $scratchRoot "CultMath"
$releaseTarPath = Join-Path $scratchRoot "gjallar-source-release.tar"
$cultLibTarPath = Join-Path $scratchRoot "cultlib-source-release.tar"
$cultMathTarPath = Join-Path $scratchRoot "cultmath-source-release.tar"
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

$cultLibCommit = (git -C $CultLibRoot rev-parse HEAD).Trim()
if ([string]::IsNullOrWhiteSpace($cultLibCommit)) {
  throw "Could not determine CultLib git revision."
}

$cultMathCommit = (git -C $CultMathRoot rev-parse HEAD).Trim()
if ([string]::IsNullOrWhiteSpace($cultMathCommit)) {
  throw "Could not determine CultMath git revision."
}

New-Item -ItemType Directory -Force -Path $scratchRoot | Out-Null
Remove-ScratchPath -ScratchRoot $scratchRoot -TargetPath $publishDir
Export-GitArchive -RepoRoot $GjallarRoot -Ref $sourceRef -Destination $releaseSource -TarPath $releaseTarPath
Export-GitArchive -RepoRoot $CultLibRoot -Ref $cultLibCommit -Destination $cultLibRelease -TarPath $cultLibTarPath
Export-GitArchive -RepoRoot $CultMathRoot -Ref $cultMathCommit -Destination $cultMathRelease -TarPath $cultMathTarPath

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
  "cultLibCommit=$cultLibCommit"
  "cultMathCommit=$cultMathCommit"
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
Environment=GJALLAR_ODIN_CULTMESH_URI=$OdinCultMeshUri
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

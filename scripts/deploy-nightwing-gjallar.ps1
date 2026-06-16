param(
  [string] $GjallarRoot = "E:\Projects\Gjallar",
  [string] $SshTarget = "nwroot",
  [string] $RemoteDir = "/opt/gamecult/gjallar",
  [string] $IdunnRudpHealth = "10.77.0.2:17870",
  [string] $IdunnDaemon = "nightwing-gjallar",
  [string] $IdunnHealthContract = "gjallar.cultnet-rudp-framebuffer-composition-health"
)

$ErrorActionPreference = "Stop"

$publishDir = Join-Path $GjallarRoot "scratch\publish\gjallar"
$tarPath = Join-Path $GjallarRoot "scratch\gjallar-publish.tar"
$projectPath = Join-Path $GjallarRoot "src\Gjallar\Gjallar.csproj"
$manifestPath = Join-Path $publishDir "gamecult-gjallar-deploy-manifest.txt"

$commit = (git -C $GjallarRoot rev-parse HEAD).Trim()
if ([string]::IsNullOrWhiteSpace($commit)) {
  throw "Could not determine Gjallar git revision."
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

$dropIn = @"
[Service]
Environment=GJALLAR_IDUNN_RUDP_HEALTH=$IdunnRudpHealth
Environment=GJALLAR_IDUNN_DAEMON=$IdunnDaemon
Environment=GJALLAR_IDUNN_HEALTH_CONTRACT=$IdunnHealthContract
"@
$dropInBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($dropIn))

ssh.exe $SshTarget "mkdir -p '$RemoteDir' /etc/systemd/system/gjallar.service.d && tar -xf /tmp/gjallar-publish.tar -C '$RemoteDir' && chmod +x '$RemoteDir/Gjallar' && printf '%s' '$dropInBase64' | base64 -d > /etc/systemd/system/gjallar.service.d/idunn-rudp-health.conf && systemctl daemon-reload && systemctl restart gjallar.service && systemctl is-active --quiet gjallar.service"
if ($LASTEXITCODE -ne 0) {
  throw "remote deploy or restart failed with exit code $LASTEXITCODE"
}

Write-Host "Gjallar deployed to Nightwing."
Write-Host "  gitCommit=$commit"
Write-Host "  sha256=$hash"

param(
  [string] $GjallarRoot = "E:\Projects\Gjallar",
  [string] $SshTarget = "nwroot",
  [string] $RemoteDir = "/opt/gamecult/gjallar"
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

ssh.exe $SshTarget "mkdir -p '$RemoteDir' && tar -xf /tmp/gjallar-publish.tar -C '$RemoteDir' && chmod +x '$RemoteDir/Gjallar' && systemctl restart gjallar.service && systemctl is-active --quiet gjallar.service"
if ($LASTEXITCODE -ne 0) {
  throw "remote deploy or restart failed with exit code $LASTEXITCODE"
}

Write-Host "Gjallar deployed to Nightwing."
Write-Host "  gitCommit=$commit"
Write-Host "  sha256=$hash"

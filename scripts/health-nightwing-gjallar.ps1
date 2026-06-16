param(
  [string] $GjallarRoot = "E:\Projects\Gjallar",
  [string] $SshTarget = "nightwing",
  [string] $RemoteManifest = "/opt/gamecult/gjallar/gamecult-gjallar-deploy-manifest.txt",
  [string] $RemoteStatus = "/var/log/gjallar.status",
  [int] $MaxStatusAgeSeconds = 120,
  [int] $MinCatalogProviders = 1,
  [int] $MinComposedProviders = 1
)

$ErrorActionPreference = "Stop"

function Fail-IdunnHealth {
  param(
    [string] $State,
    [string] $Message
  )

  Write-Output "idunn.health.state=$State"
  throw $Message
}

ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "systemctl is-active --quiet gjallar.service"
if ($LASTEXITCODE -ne 0) {
  Fail-IdunnHealth -State "failed" -Message "gjallar.service is not active on Nightwing."
}

$localCommit = (git -C $GjallarRoot rev-parse HEAD).Trim()
if ([string]::IsNullOrWhiteSpace($localCommit)) {
  throw "Could not determine local Gjallar git revision."
}

$manifest = ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "cat '$RemoteManifest' 2>/dev/null"
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($manifest)) {
  Fail-IdunnHealth -State "stale-deployment" -Message "Nightwing Gjallar deploy manifest is missing."
}

$remoteCommit = ""
foreach ($line in $manifest) {
  if ($line -like "gitCommit=*") {
    $remoteCommit = $line.Substring("gitCommit=".Length).Trim()
  }
}

if ($remoteCommit -ne $localCommit) {
  Fail-IdunnHealth -State "stale-deployment" -Message "Nightwing Gjallar deploy is stale. remote=$remoteCommit local=$localCommit"
}

$statusJson = ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "cat '$RemoteStatus' 2>/dev/null"
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($statusJson)) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Gjallar status witness is missing at $RemoteStatus."
}

$status = $statusJson | ConvertFrom-Json
if ($status.schema -ne "gamecult.gjallar.frame.v1") {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Gjallar status witness has unexpected schema '$($status.schema)'."
}

$updatedAt = [DateTimeOffset]::Parse($status.updatedAtUtc)
$ageSeconds = ([DateTimeOffset]::UtcNow - $updatedAt.ToUniversalTime()).TotalSeconds
if ($ageSeconds -gt $MaxStatusAgeSeconds) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Gjallar status witness is stale ($([math]::Round($ageSeconds))s old)."
}

if ($status.status -ne "rendered") {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Gjallar framebuffer status is '$($status.status)', expected rendered."
}

if ($status.receive.status -ne "catalog-composed") {
  $detail = $status.receive.error
  if ([string]::IsNullOrWhiteSpace($detail)) {
    $detail = $status.receive.providerFetchError
  }
  Fail-IdunnHealth -State "dependency-unavailable" -Message "Nightwing Gjallar receive status is '$($status.receive.status)', expected catalog-composed. $detail"
}

if ([int]$status.receive.catalogProviders -lt $MinCatalogProviders) {
  Fail-IdunnHealth -State "dependency-unavailable" -Message "Nightwing Gjallar catalog is empty: catalogProviders=$($status.receive.catalogProviders), expected at least $MinCatalogProviders."
}

if ([int]$status.receive.composedProviders -lt $MinComposedProviders) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Gjallar composed no provider panels: composedProviders=$($status.receive.composedProviders), expected at least $MinComposedProviders."
}

if ($null -eq $status.idunnRudpHealth -or $status.idunnRudpHealth.status -ne "published") {
  $detail = if ($null -eq $status.idunnRudpHealth) { "missing idunnRudpHealth status" } else { $status.idunnRudpHealth.error }
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Gjallar has not published daemon-owned Idunn RUDP health. $detail"
}

Write-Host "Nightwing Gjallar is active, deployed at $localCommit, and composing $($status.receive.composedProviders)/$($status.receive.catalogProviders) providers."

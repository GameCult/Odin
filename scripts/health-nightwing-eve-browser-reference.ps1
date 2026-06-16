param(
  [string] $SshTarget = "nightwing",
  [string] $ExpectedDocumentRoot = "/opt/gamecult/eve-browser-reference",
  [string] $RemoteCultCacheWitness = "/var/lib/gamecult/eve-browser-reference/cultcache/eve-browser-reference.service.cc",
  [int] $MaxWitnessAgeSeconds = 120
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

ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "systemctl is-active --quiet nightwing-eve-browser-reference.service"
if ($LASTEXITCODE -ne 0) {
  Fail-IdunnHealth -State "failed" -Message "nightwing-eve-browser-reference.service is not active on Nightwing."
}

$healthJson = ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "curl -fsS http://127.0.0.1:8891/health"
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($healthJson)) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference /health probe failed."
}

$health = $healthJson | ConvertFrom-Json
if (-not $health.ok) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference reported ok=false."
}

if ($health.documentRoot -ne $ExpectedDocumentRoot) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference document root is '$($health.documentRoot)', expected '$ExpectedDocumentRoot'."
}

if ($health.transport -ne "static-http-lowering") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference transport is '$($health.transport)', expected 'static-http-lowering'."
}

if ($null -eq $health.cultCacheWitness) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference did not report cultCacheWitness status."
}

if ($health.cultCacheWitness.status -ne "published") {
  $detail = $health.cultCacheWitness.error
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference has not published its daemon-owned CultCache witness. $detail"
}

if ($health.cultCacheWitness.path -ne $RemoteCultCacheWitness) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference CultCache witness path is '$($health.cultCacheWitness.path)', expected '$RemoteCultCacheWitness'."
}

$requiredCultCacheSchemas = @(
  "mimir.eve_browser_reference_manifest.v1",
  "mimir.eve_browser_reference_static_surface.v1",
  "mimir.eve_browser_reference_command_boundary.v1",
  "mimir.eve_browser_reference_transport_profile.v1",
  "idunn.daemon_health.v1"
)

$publishedCultCacheSchemas = @($health.cultCacheWitness.publishedSchemas)
foreach ($schema in $requiredCultCacheSchemas) {
  if ($publishedCultCacheSchemas -notcontains $schema) {
    Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference CultCache witness is missing schema '$schema'."
  }
}

if ($null -eq $health.idunnRudpHealth -or $health.idunnRudpHealth.status -ne "published") {
  $detail = if ($null -eq $health.idunnRudpHealth) { "missing idunnRudpHealth status" } else { $health.idunnRudpHealth.error }
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference has not published daemon-owned Idunn RUDP health. $detail"
}

$cultCacheStat = ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "stat -c '%s %Y %U %G' '$RemoteCultCacheWitness' 2>/dev/null"
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($cultCacheStat)) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference CultCache witness is missing at $RemoteCultCacheWitness."
}

$cultCacheParts = $cultCacheStat.Trim() -split '\s+'
if ($cultCacheParts.Length -lt 4) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference CultCache witness stat output was malformed: $cultCacheStat"
}

$cultCacheSize = [int64]$cultCacheParts[0]
$cultCacheModifiedUtc = [DateTimeOffset]::FromUnixTimeSeconds([int64]$cultCacheParts[1])
$cultCacheOwner = $cultCacheParts[2]
$cultCacheGroup = $cultCacheParts[3]
$cultCacheAgeSeconds = ([DateTimeOffset]::UtcNow - $cultCacheModifiedUtc).TotalSeconds
if ($cultCacheSize -le 0) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference CultCache witness at $RemoteCultCacheWitness is empty."
}

if ($cultCacheAgeSeconds -gt $MaxWitnessAgeSeconds) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference CultCache witness is stale ($([math]::Round($cultCacheAgeSeconds))s old)."
}

if ($cultCacheOwner -ne "metacrat" -or $cultCacheGroup -ne "metacrat") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference CultCache witness owner is ${cultCacheOwner}:$cultCacheGroup, expected metacrat:metacrat."
}

Write-Host "Nightwing Eve browser reference is active, publishing its daemon-owned CultCache witness, and keeping Idunn RUDP health live."

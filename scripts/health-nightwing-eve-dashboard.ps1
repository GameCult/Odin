param(
  [string] $SshTarget = "nightwing",
  [string] $RemoteCultMeshState = "/var/lib/gamecult/eve-dashboard/cultmesh/eve-dashboard.ccmp",
  [string] $RemoteCultCacheWitness = "/var/lib/gamecult/eve-dashboard/cultcache/eve-dashboard.service.cc",
  [string] $LegacyWindowsWitness = "/opt/gamecult/eve-dashboard/E:\\Projects\\Mimir\\state\\eve-dashboard-cultnet.ccmp",
  [int] $MaxWitnessAgeSeconds = 120,
  [int] $MinProviders = 1
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

ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "systemctl is-active --quiet nightwing-eve-dashboard.service"
if ($LASTEXITCODE -ne 0) {
  Fail-IdunnHealth -State "failed" -Message "nightwing-eve-dashboard.service is not active on Nightwing."
}

$healthJson = ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "curl -fsS http://127.0.0.1:8795/health"
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($healthJson)) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard /health probe failed."
}

$health = $healthJson | ConvertFrom-Json
if (-not $health.ok) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard reported ok=false."
}

if ($health.cultMeshDocument -ne "mimir.eve_dashboard_state") {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard reported unexpected CultMesh document '$($health.cultMeshDocument)'."
}

if ($null -eq $health.cultMeshWitness) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard did not report cultMeshWitness status."
}

if ($health.cultMeshWitness.status -ne "published") {
  $detail = $health.cultMeshWitness.error
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard has not published its CultMesh state witness. $detail"
}

if ($health.cultMeshWitness.path -ne $RemoteCultMeshState) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard CultMesh state path is '$($health.cultMeshWitness.path)', expected '$RemoteCultMeshState'."
}

if ($null -eq $health.cultCacheWitness) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard did not report cultCacheWitness status."
}

if ($health.cultCacheWitness.status -ne "published") {
  $detail = $health.cultCacheWitness.error
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard has not published its daemon-owned CultCache witness. $detail"
}

if ($health.cultCacheWitness.path -ne $RemoteCultCacheWitness) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard CultCache witness path is '$($health.cultCacheWitness.path)', expected '$RemoteCultCacheWitness'."
}

$requiredCultMeshSchemas = @(
  "mimir.eve_dashboard_state.v1"
)

$publishedCultMeshSchemas = @($health.cultMeshWitness.publishedSchemas)
foreach ($schema in $requiredCultMeshSchemas) {
  if ($publishedCultMeshSchemas -notcontains $schema) {
    Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard CultMesh witness is missing schema '$schema'."
  }
}

$requiredCultCacheSchemas = @(
  "mimir.eve_dashboard_manifest.v1",
  "mimir.eve_dashboard_command_boundary.v1",
  "mimir.eve_dashboard_transport_profile.v1",
  "idunn.daemon_health.v1"
)

$publishedCultCacheSchemas = @($health.cultCacheWitness.publishedSchemas)
foreach ($schema in $requiredCultCacheSchemas) {
  if ($publishedCultCacheSchemas -notcontains $schema) {
    Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard CultCache witness is missing schema '$schema'."
  }
}

if ($null -eq $health.idunnRudpHealth -or $health.idunnRudpHealth.status -ne "published") {
  $detail = if ($null -eq $health.idunnRudpHealth) { "missing idunnRudpHealth status" } else { $health.idunnRudpHealth.error }
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard has not published daemon-owned Idunn RUDP health. $detail"
}

if ([int]$health.providers -lt $MinProviders) {
  Fail-IdunnHealth -State "dependency-unavailable" -Message "Nightwing Eve dashboard reports too few providers: providers=$($health.providers), expected at least $MinProviders."
}

$cultMeshStateStat = ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "stat -c '%s %Y' '$RemoteCultMeshState' 2>/dev/null"
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($cultMeshStateStat)) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard CultMesh state witness is missing at $RemoteCultMeshState."
}

$cultMeshStateParts = $cultMeshStateStat.Trim() -split '\s+'
if ($cultMeshStateParts.Length -lt 2) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard CultMesh state stat output was malformed: $cultMeshStateStat"
}

$cultMeshStateSize = [int64]$cultMeshStateParts[0]
$cultMeshStateModifiedUtc = [DateTimeOffset]::FromUnixTimeSeconds([int64]$cultMeshStateParts[1])
$cultMeshStateAgeSeconds = ([DateTimeOffset]::UtcNow - $cultMeshStateModifiedUtc).TotalSeconds
if ($cultMeshStateSize -le 0) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard CultMesh state witness at $RemoteCultMeshState is empty."
}

if ($cultMeshStateAgeSeconds -gt $MaxWitnessAgeSeconds) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard CultMesh state witness is stale ($([math]::Round($cultMeshStateAgeSeconds))s old)."
}

$cultCacheStat = ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "stat -c '%s %Y' '$RemoteCultCacheWitness' 2>/dev/null"
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($cultCacheStat)) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard CultCache witness is missing at $RemoteCultCacheWitness."
}

$cultCacheParts = $cultCacheStat.Trim() -split '\s+'
if ($cultCacheParts.Length -lt 2) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard CultCache witness stat output was malformed: $cultCacheStat"
}

$cultCacheSize = [int64]$cultCacheParts[0]
$cultCacheModifiedUtc = [DateTimeOffset]::FromUnixTimeSeconds([int64]$cultCacheParts[1])
$cultCacheAgeSeconds = ([DateTimeOffset]::UtcNow - $cultCacheModifiedUtc).TotalSeconds
if ($cultCacheSize -le 0) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard CultCache witness at $RemoteCultCacheWitness is empty."
}

if ($cultCacheAgeSeconds -gt $MaxWitnessAgeSeconds) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard CultCache witness is stale ($([math]::Round($cultCacheAgeSeconds))s old)."
}

$legacyWitnessStillExists = ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "test ! -e '$LegacyWindowsWitness'"
if ($LASTEXITCODE -ne 0) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard still has the legacy Windows-shaped witness file at $LegacyWindowsWitness."
}

Write-Host "Nightwing Eve dashboard is active, publishing daemon-owned CultMesh witness state, and advertising $($health.providers) providers."

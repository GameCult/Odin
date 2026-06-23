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

function Invoke-SshText {
  param(
    [string] $Command,
    [string] $FailureMessage,
    [string] $FailureState = "failed"
  )

  $output = ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget $Command
  if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($output)) {
    Fail-IdunnHealth -State $FailureState -Message $FailureMessage
  }

  return $output
}

function Get-CultCacheInspection {
  param(
    [string] $WitnessCopyPath,
    [string] $InspectorPackageJson,
    [string] $Mode
  )

  $nodeScript = @'
const fs = require("fs");
const { createRequire } = require("module");

const witnessCopyPath = process.argv[2];
const inspectorPackageJson = process.argv[3];
const mode = process.argv[4];
const requireCultCacheInspection = createRequire(inspectorPackageJson);
const { inspectCultCacheBytes } = requireCultCacheInspection("cultcache-ts");
const inspection = inspectCultCacheBytes(witnessCopyPath, fs.readFileSync(witnessCopyPath));

function firstRecord(schemaName) {
  return inspection.records.find((record) => record.schemaName === schemaName) ?? null;
}

function allRecords(schemaName) {
  return inspection.records.filter((record) => record.schemaName === schemaName);
}

function previewArray(record) {
  return Array.isArray(record && record.payloadPreview) ? record.payloadPreview : [];
}

function normalizeDashboardState(record) {
  if (!record) {
    return null;
  }

  const preview = previewArray(record);
  const surface = Array.isArray(preview[7]) ? preview[7] : [];
  const root = Array.isArray(surface[3]) ? surface[3] : [];
  const children = Array.isArray(root[11]) ? root[11] : [];
  return {
    schemaName: record.schemaName,
    storedAt: record.storedAt,
    providerId: preview[0] ?? "",
    title: preview[1] ?? "",
    version: preview[2] ?? 0,
    updatedAtUtc: preview[3] ?? "",
    selectedNodeId: preview[4] ?? "",
    lutPreset: preview[5] ?? "",
    nodeCount: Array.isArray(preview[6]) ? preview[6].length : 0,
    surfaceSchema: surface[0] ?? "",
    surfaceId: surface[1] ?? "",
    surfaceTitle: surface[2] ?? "",
    surfaceRootKind: root[1] ?? "",
    surfaceChildCount: children.length,
    assetCount: Array.isArray(surface[4]) ? surface[4].length : 0,
  };
}

function normalizeManifest(record) {
  if (!record) {
    return null;
  }

  const preview = previewArray(record);
  return {
    schemaName: record.schemaName,
    storedAt: record.storedAt,
    providerId: preview[0] ?? "",
    title: preview[1] ?? "",
    description: preview[2] ?? "",
    version: preview[3] ?? "",
    endpoint: preview[4] ?? "",
    capabilityCount: Array.isArray(preview[5]) ? preview[5].length : 0,
    usesCultMesh: preview[6] === true,
    transport: preview[7] ?? "",
  };
}

function normalizeCommandBoundary(record) {
  if (!record) {
    return null;
  }

  const preview = previewArray(record);
  return {
    schemaName: record.schemaName,
    storedAt: record.storedAt,
    daemonId: preview[0] ?? "",
    updatedAtUtc: preview[1] ?? "",
    mode: preview[2] ?? "",
    writesAccepted: preview[3] === true,
    operatorInputAuthority: preview[4] ?? "",
    lifecycleAuthority: preview[5] ?? "",
    acceptedCommands: Array.isArray(preview[6]) ? preview[6] : [],
    rejectedCommands: Array.isArray(preview[7]) ? preview[7] : [],
  };
}

function normalizeTransportProfile(record) {
  if (!record) {
    return null;
  }

  const preview = previewArray(record);
  return {
    schemaName: record.schemaName,
    storedAt: record.storedAt,
    daemonId: preview[0] ?? "",
    updatedAtUtc: preview[1] ?? "",
    currentState: preview[2] ?? "",
    inputTransport: preview[3] ?? "",
    outputTransport: preview[4] ?? "",
    healthContract: preview[5] ?? "",
    idunnRudpHealth: preview[6] ?? "",
    witnessPath: preview[7] ?? "",
    currentCutLine: preview[8] ?? "",
  };
}

function normalizeDaemonHealth(record) {
  if (!record) {
    return null;
  }

  const preview = previewArray(record);
  return {
    schemaName: record.schemaName,
    storedAt: record.storedAt,
    daemonId: preview[0] ?? "",
    state: preview[1] ?? "",
    detail: preview[2] ?? "",
    observedAt: preview[3] ?? "",
    healthContract: preview[4] ?? "",
    publicationSource: preview[5] ?? "",
    transport: preview[6] ?? "",
  };
}

let payload;
switch (mode) {
  case "dashboard-state":
    payload = {
      format: inspection.format,
      schemaVersions: inspection.catalog.map((entry) => entry.schemaVersion),
      dashboardState: normalizeDashboardState(firstRecord("mimir.eve_dashboard_state")),
    };
    break;
  case "dashboard-boundary":
    payload = {
      format: inspection.format,
      schemaVersions: inspection.catalog.map((entry) => entry.schemaVersion),
      brokerManifest: normalizeManifest(allRecords("mimir.eve_dashboard_manifest").find((record) => record.key === "eve.dashboard.broker") ?? null),
      providerManifests: allRecords("mimir.eve_dashboard_manifest")
        .filter((record) => record.key !== "eve.dashboard.broker")
        .map(normalizeManifest),
      commandBoundary: normalizeCommandBoundary(firstRecord("mimir.eve_dashboard_command_boundary")),
      transportProfile: normalizeTransportProfile(firstRecord("mimir.eve_dashboard_transport_profile")),
      daemonHealthRecords: allRecords("idunn.daemon_health").map(normalizeDaemonHealth),
    };
    break;
  default:
    throw new Error(`Unknown inspection mode '${mode}'.`);
}

console.log(JSON.stringify(payload));
'@

  $inspectionJson = $nodeScript | node - $WitnessCopyPath $InspectorPackageJson $Mode
  if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($inspectionJson)) {
    Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard witness inspection failed for mode '$Mode'."
  }

  try {
    return $inspectionJson | ConvertFrom-Json
  } catch {
    Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard witness inspection returned malformed JSON for mode '$Mode'."
  }
}

ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "systemctl is-active --quiet nightwing-eve-dashboard.service"
if ($LASTEXITCODE -ne 0) {
  Fail-IdunnHealth -State "failed" -Message "nightwing-eve-dashboard.service is not active on Nightwing."
}

$cultMeshStateStat = Invoke-SshText `
  -Command "stat -c '%s %Y' '$RemoteCultMeshState' 2>/dev/null" `
  -FailureMessage "Nightwing Eve dashboard CultMesh state witness is missing at $RemoteCultMeshState."

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

$cultCacheStat = Invoke-SshText `
  -Command "stat -c '%s %Y' '$RemoteCultCacheWitness' 2>/dev/null" `
  -FailureMessage "Nightwing Eve dashboard CultCache witness is missing at $RemoteCultCacheWitness."

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

try {
  $cultCacheInspectorPackageJson = (Resolve-Path (Join-Path $PSScriptRoot "..\..\CultLib\packages\cultcache-ts\package.json")).Path
} catch {
  Fail-IdunnHealth -State "failed" -Message "CultCache inspector package is missing from the local Odin body."
}

$cultMeshWitnessCopy = Join-Path ([System.IO.Path]::GetTempPath()) ("nightwing-eve-dashboard-state-" + [Guid]::NewGuid().ToString("N") + ".ccmp")
$cultCacheWitnessCopy = Join-Path ([System.IO.Path]::GetTempPath()) ("nightwing-eve-dashboard-boundary-" + [Guid]::NewGuid().ToString("N") + ".cc")
try {
  $remoteCultMeshBase64 = Invoke-SshText `
    -Command "base64 -w0 '$RemoteCultMeshState' 2>/dev/null" `
    -FailureMessage "Nightwing Eve dashboard CultMesh witness could not be copied from Nightwing."
  [System.IO.File]::WriteAllBytes($cultMeshWitnessCopy, [Convert]::FromBase64String($remoteCultMeshBase64.Trim()))
  $cultMeshInspection = Get-CultCacheInspection -WitnessCopyPath $cultMeshWitnessCopy -InspectorPackageJson $cultCacheInspectorPackageJson -Mode "dashboard-state"

  $remoteCultCacheBase64 = Invoke-SshText `
    -Command "base64 -w0 '$RemoteCultCacheWitness' 2>/dev/null" `
    -FailureMessage "Nightwing Eve dashboard CultCache witness could not be copied from Nightwing."
  [System.IO.File]::WriteAllBytes($cultCacheWitnessCopy, [Convert]::FromBase64String($remoteCultCacheBase64.Trim()))
  $cultCacheInspection = Get-CultCacheInspection -WitnessCopyPath $cultCacheWitnessCopy -InspectorPackageJson $cultCacheInspectorPackageJson -Mode "dashboard-boundary"
} catch [FormatException] {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard witness copy was not valid base64."
} finally {
  foreach ($temporaryWitness in @($cultMeshWitnessCopy, $cultCacheWitnessCopy)) {
    if (Test-Path $temporaryWitness) {
      Remove-Item $temporaryWitness -Force -ErrorAction SilentlyContinue
    }
  }
}

if ($cultMeshInspection.format -ne "cultcache.store.v1") {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard CultMesh witness has unexpected format '$($cultMeshInspection.format)'."
}

$requiredCultMeshSchemas = @(
  "mimir.eve_dashboard_state.v1"
)

$publishedCultMeshSchemas = @($cultMeshInspection.schemaVersions)
foreach ($schema in $requiredCultMeshSchemas) {
  if ($publishedCultMeshSchemas -notcontains $schema) {
    Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard CultMesh witness is missing schema '$schema'."
  }
}

if ($null -eq $cultMeshInspection.dashboardState) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard CultMesh witness is missing its dashboard-state record."
}

if ([string]::IsNullOrWhiteSpace($cultMeshInspection.dashboardState.providerId)) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard CultMesh state is missing a providerId."
}

if ([string]::IsNullOrWhiteSpace($cultMeshInspection.dashboardState.selectedNodeId)) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard CultMesh state is missing a selectedNodeId."
}

if ([string]::IsNullOrWhiteSpace($cultMeshInspection.dashboardState.title)) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard CultMesh state is missing a title."
}

if ([int]$cultMeshInspection.dashboardState.nodeCount -le 0) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard CultMesh state has no nodes."
}

if ($cultCacheInspection.format -ne "cultcache.store.v1") {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard CultCache witness has unexpected format '$($cultCacheInspection.format)'."
}

$requiredCultCacheSchemas = @(
  "mimir.eve_dashboard_manifest.v1",
  "mimir.eve_dashboard_command_boundary.v1",
  "mimir.eve_dashboard_transport_profile.v1",
  "idunn.daemon_health.v1"
)

$publishedCultCacheSchemas = @($cultCacheInspection.schemaVersions)
foreach ($schema in $requiredCultCacheSchemas) {
  if ($publishedCultCacheSchemas -notcontains $schema) {
    Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard CultCache witness is missing schema '$schema'."
  }
}

if ($null -eq $cultCacheInspection.brokerManifest) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard CultCache witness is missing its broker manifest record."
}

if ($cultCacheInspection.brokerManifest.providerId -ne "eve.dashboard.broker") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard broker manifest advertises provider '$($cultCacheInspection.brokerManifest.providerId)', expected 'eve.dashboard.broker'."
}

if ([int]@($cultCacheInspection.providerManifests).Count -lt $MinProviders) {
  Fail-IdunnHealth -State "dependency-unavailable" -Message "Nightwing Eve dashboard has too few provider manifests in its daemon-owned witness: providers=$(@($cultCacheInspection.providerManifests).Count), expected at least $MinProviders."
}

if ($null -eq $cultCacheInspection.commandBoundary) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard CultCache witness is missing its command boundary record."
}

if ($cultCacheInspection.commandBoundary.mode -ne "operator-write-through-broker") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard command boundary mode is '$($cultCacheInspection.commandBoundary.mode)', expected 'operator-write-through-broker'."
}

if (-not $cultCacheInspection.commandBoundary.writesAccepted) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard command boundary unexpectedly rejects writes."
}

if ($cultCacheInspection.commandBoundary.lifecycleAuthority -notmatch "nightwing-eve-dashboard\.service") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard lifecycle authority is '$($cultCacheInspection.commandBoundary.lifecycleAuthority)', expected the systemd service to remain the compatibility lifecycle witness."
}

if ($null -eq $cultCacheInspection.transportProfile) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard CultCache witness is missing its transport profile record."
}

if ($cultCacheInspection.transportProfile.currentState -ne "partial-rudp-health-and-provider-store-live") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard transport profile state is '$($cultCacheInspection.transportProfile.currentState)', expected 'partial-rudp-health-and-provider-store-live'."
}

if ($cultCacheInspection.transportProfile.inputTransport -ne "compatibility.http-websocket-client-lowering + compatibility.websocket-binary-command-lowering") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard input transport is '$($cultCacheInspection.transportProfile.inputTransport)', expected 'compatibility.http-websocket-client-lowering + compatibility.websocket-binary-command-lowering'."
}

if ($cultCacheInspection.transportProfile.outputTransport -ne "daemon-owned-cultcache-provider-store + daemon-published-rudp-health") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard output transport is '$($cultCacheInspection.transportProfile.outputTransport)', expected 'daemon-owned-cultcache-provider-store + daemon-published-rudp-health'."
}

if ($cultCacheInspection.transportProfile.idunnRudpHealth -ne "idunn.health-published-separately") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard transport profile Idunn health publication is '$($cultCacheInspection.transportProfile.idunnRudpHealth)', expected 'idunn.health-published-separately'."
}

if ($cultCacheInspection.transportProfile.witnessPath -ne $RemoteCultCacheWitness) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard transport profile witness path is '$($cultCacheInspection.transportProfile.witnessPath)', expected '$RemoteCultCacheWitness'."
}

$matchingHealthRecord = @($cultCacheInspection.daemonHealthRecords | Where-Object {
    $_.healthContract -eq $cultCacheInspection.transportProfile.healthContract
  }) | Select-Object -First 1
if ($null -eq $matchingHealthRecord) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard has no daemon-health record matching contract '$($cultCacheInspection.transportProfile.healthContract)'."
}

if ($matchingHealthRecord.state -ne "healthy") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard daemon-health state is '$($matchingHealthRecord.state)', expected 'healthy'."
}

if ($matchingHealthRecord.publicationSource -ne "daemon-published") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard daemon-health publication source is '$($matchingHealthRecord.publicationSource)', expected 'daemon-published'."
}

if ($matchingHealthRecord.transport -ne "cultnet.transport.rudp.v0") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard daemon-health transport is '$($matchingHealthRecord.transport)', expected 'cultnet.transport.rudp.v0'."
}

try {
  $matchingHealthObservedAtUtc = [DateTimeOffset]::Parse($matchingHealthRecord.observedAt)
} catch {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard daemon-health observedAt is malformed: '$($matchingHealthRecord.observedAt)'."
}

$matchingHealthAgeSeconds = ([DateTimeOffset]::UtcNow - $matchingHealthObservedAtUtc.ToUniversalTime()).TotalSeconds
if ($matchingHealthAgeSeconds -gt $MaxWitnessAgeSeconds) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve dashboard daemon-health record is stale ($([math]::Round($matchingHealthAgeSeconds))s old)."
}

$legacyWitnessStillExists = ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "test ! -e '$LegacyWindowsWitness'"
if ($LASTEXITCODE -ne 0) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve dashboard still has the legacy Windows-shaped witness file at $LegacyWindowsWitness."
}

Write-Host "Nightwing Eve dashboard is active, its CultMesh and CultCache witnesses are fresh, and the daemon-owned boundary store advertises $(@($cultCacheInspection.providerManifests).Count) providers."

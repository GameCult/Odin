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
    [string] $InspectorPackageJson
  )

  $nodeScript = @'
const fs = require("fs");
const { createRequire } = require("module");

const witnessCopyPath = process.argv[2];
const inspectorPackageJson = process.argv[3];
const requireCultCacheInspection = createRequire(inspectorPackageJson);
const { inspectCultCacheBytes } = requireCultCacheInspection("cultcache-ts");
const inspection = inspectCultCacheBytes(witnessCopyPath, fs.readFileSync(witnessCopyPath));

function firstRecord(schemaName) {
  return inspection.records.find((record) => record.schemaName === schemaName) ?? null;
}

function previewArray(record) {
  return Array.isArray(record?.payloadPreview) ? record.payloadPreview : [];
}

function normalizeRecord(schemaName) {
  const record = firstRecord(schemaName);
  if (!record) {
    return null;
  }

  const preview = previewArray(record);
  switch (schemaName) {
    case "mimir.eve_browser_reference_manifest":
      return {
        schemaName: record.schemaName,
        storedAt: record.storedAt,
        daemonId: preview[0] ?? "",
        siteTitle: preview[1] ?? "",
        documentRoot: preview[2] ?? "",
        entryPoint: preview[3] ?? "",
        port: preview[4] ?? 0,
        assetCount: Array.isArray(preview[5]) ? preview[5].length : 0,
        surfaceTransport: preview[6] ?? "",
      };
    case "mimir.eve_browser_reference_static_surface":
      return {
        schemaName: record.schemaName,
        storedAt: record.storedAt,
        daemonId: preview[0] ?? "",
        updatedAtUtc: preview[1] ?? "",
        documentRoot: preview[2] ?? "",
        entryPoint: preview[3] ?? "",
        assetCount: Array.isArray(preview[4]) ? preview[4].length : 0,
        fixtureCount: Array.isArray(preview[5]) ? preview[5].length : 0,
        currentCutLine: preview[6] ?? "",
      };
    case "mimir.eve_browser_reference_command_boundary":
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
    case "mimir.eve_browser_reference_transport_profile":
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
    case "idunn.daemon_health":
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
    default:
      return {
        schemaName: record.schemaName,
        storedAt: record.storedAt,
      };
  }
}

console.log(JSON.stringify({
  format: inspection.format,
  catalogSchemaVersions: inspection.catalog.map((entry) => entry.schemaVersion),
  manifest: normalizeRecord("mimir.eve_browser_reference_manifest"),
  staticSurface: normalizeRecord("mimir.eve_browser_reference_static_surface"),
  commandBoundary: normalizeRecord("mimir.eve_browser_reference_command_boundary"),
  transportProfile: normalizeRecord("mimir.eve_browser_reference_transport_profile"),
  daemonHealth: normalizeRecord("idunn.daemon_health"),
}));
'@

  $inspectionJson = $nodeScript | node - $WitnessCopyPath $InspectorPackageJson
  if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($inspectionJson)) {
    Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference CultCache witness inspection failed."
  }

  try {
    return $inspectionJson | ConvertFrom-Json
  } catch {
    Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference CultCache inspection returned malformed JSON."
  }
}

ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $SshTarget "systemctl is-active --quiet nightwing-eve-browser-reference.service"
if ($LASTEXITCODE -ne 0) {
  Fail-IdunnHealth -State "failed" -Message "nightwing-eve-browser-reference.service is not active on Nightwing."
}

$cultCacheStat = Invoke-SshText `
  -Command "stat -c '%s %Y %U %G' '$RemoteCultCacheWitness' 2>/dev/null" `
  -FailureMessage "Nightwing Eve browser reference CultCache witness is missing at $RemoteCultCacheWitness."

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

try {
  $cultCacheInspectorPackageJson = (Resolve-Path (Join-Path $PSScriptRoot "..\..\CultLib\packages\cultcache-ts\package.json")).Path
} catch {
  Fail-IdunnHealth -State "failed" -Message "CultCache inspector package is missing from the local Odin body."
}

$cultCacheWitnessCopy = Join-Path ([System.IO.Path]::GetTempPath()) ("nightwing-eve-browser-reference-" + [Guid]::NewGuid().ToString("N") + ".cc")
try {
  $remoteCultCacheBase64 = Invoke-SshText `
    -Command "base64 -w0 '$RemoteCultCacheWitness' 2>/dev/null" `
    -FailureMessage "Nightwing Eve browser reference CultCache witness could not be copied from Nightwing."
  [System.IO.File]::WriteAllBytes($cultCacheWitnessCopy, [Convert]::FromBase64String($remoteCultCacheBase64.Trim()))
  $inspection = Get-CultCacheInspection -WitnessCopyPath $cultCacheWitnessCopy -InspectorPackageJson $cultCacheInspectorPackageJson
} catch [FormatException] {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference CultCache witness copy was not valid base64."
} finally {
  if (Test-Path $cultCacheWitnessCopy) {
    Remove-Item $cultCacheWitnessCopy -Force -ErrorAction SilentlyContinue
  }
}

if ($inspection.format -ne "cultcache.store.v1") {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference CultCache witness has unexpected format '$($inspection.format)'."
}

$requiredCultCacheSchemas = @(
  "mimir.eve_browser_reference_manifest.v1",
  "mimir.eve_browser_reference_static_surface.v1",
  "mimir.eve_browser_reference_command_boundary.v1",
  "mimir.eve_browser_reference_transport_profile.v1",
  "idunn.daemon_health.v1"
)

$publishedCultCacheSchemas = @($inspection.catalogSchemaVersions)
foreach ($schema in $requiredCultCacheSchemas) {
  if ($publishedCultCacheSchemas -notcontains $schema) {
    Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference CultCache witness is missing schema '$schema'."
  }
}

if ($null -eq $inspection.manifest) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference CultCache witness is missing its manifest record."
}

if ($inspection.manifest.documentRoot -ne $ExpectedDocumentRoot) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference manifest document root is '$($inspection.manifest.documentRoot)', expected '$ExpectedDocumentRoot'."
}

if ($inspection.manifest.entryPoint -ne "index.html") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference manifest entry point is '$($inspection.manifest.entryPoint)', expected 'index.html'."
}

if ($inspection.manifest.surfaceTransport -ne "static-http-lowering") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference manifest transport is '$($inspection.manifest.surfaceTransport)', expected 'static-http-lowering'."
}

if ($null -eq $inspection.staticSurface) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference CultCache witness is missing its static-surface record."
}

if ($inspection.staticSurface.documentRoot -ne $ExpectedDocumentRoot) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference static-surface document root is '$($inspection.staticSurface.documentRoot)', expected '$ExpectedDocumentRoot'."
}

if ($null -eq $inspection.commandBoundary) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference CultCache witness is missing its command boundary record."
}

if ($inspection.commandBoundary.mode -ne "static-read-only-lowering") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference command boundary mode is '$($inspection.commandBoundary.mode)', expected 'static-read-only-lowering'."
}

if ($inspection.commandBoundary.writesAccepted) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference command boundary unexpectedly accepts writes."
}

if ($inspection.commandBoundary.lifecycleAuthority -ne "idunn-supervisor-command.restart") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference lifecycle authority is '$($inspection.commandBoundary.lifecycleAuthority)', expected 'idunn-supervisor-command.restart'."
}

if ($null -eq $inspection.transportProfile) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference CultCache witness is missing its transport profile record."
}

if ($inspection.transportProfile.currentState -ne "partial-rudp-health-and-provider-store-live") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference transport profile state is '$($inspection.transportProfile.currentState)', expected 'partial-rudp-health-and-provider-store-live'."
}

if ($inspection.transportProfile.inputTransport -ne "http-static-lowering") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference input transport is '$($inspection.transportProfile.inputTransport)', expected 'http-static-lowering'."
}

if ($inspection.transportProfile.outputTransport -ne "daemon-owned-cultcache-boundary-store + daemon-published-rudp-health") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference output transport is '$($inspection.transportProfile.outputTransport)', expected 'daemon-owned-cultcache-boundary-store + daemon-published-rudp-health'."
}

if ($inspection.transportProfile.healthContract -ne "nightwing.cultnet-rudp-browser-reference-health") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference health contract is '$($inspection.transportProfile.healthContract)', expected 'nightwing.cultnet-rudp-browser-reference-health'."
}

if ($inspection.transportProfile.idunnRudpHealth -ne "idunn.health-published-separately") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference transport profile Idunn health publication is '$($inspection.transportProfile.idunnRudpHealth)', expected 'idunn.health-published-separately'."
}

if ($inspection.transportProfile.witnessPath -ne $RemoteCultCacheWitness) {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference transport profile witness path is '$($inspection.transportProfile.witnessPath)', expected '$RemoteCultCacheWitness'."
}

if ($null -eq $inspection.daemonHealth) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference CultCache witness is missing its daemon-health record."
}

if ($inspection.daemonHealth.daemonId -ne "nightwing-eve-browser-reference") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference daemon-health record advertises daemon '$($inspection.daemonHealth.daemonId)', expected 'nightwing-eve-browser-reference'."
}

if ($inspection.daemonHealth.state -ne "healthy") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference daemon-health state is '$($inspection.daemonHealth.state)', expected 'healthy'."
}

if ($inspection.daemonHealth.healthContract -ne "nightwing.cultnet-rudp-browser-reference-health") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference daemon-health contract is '$($inspection.daemonHealth.healthContract)', expected 'nightwing.cultnet-rudp-browser-reference-health'."
}

if ($inspection.daemonHealth.publicationSource -ne "daemon-published") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference daemon-health publication source is '$($inspection.daemonHealth.publicationSource)', expected 'daemon-published'."
}

if ($inspection.daemonHealth.transport -ne "cultnet.transport.rudp.v0") {
  Fail-IdunnHealth -State "degraded" -Message "Nightwing Eve browser reference daemon-health transport is '$($inspection.daemonHealth.transport)', expected 'cultnet.transport.rudp.v0'."
}

try {
  $daemonHealthObservedAtUtc = [DateTimeOffset]::Parse($inspection.daemonHealth.observedAt)
} catch {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference daemon-health observedAt is malformed: '$($inspection.daemonHealth.observedAt)'."
}

$daemonHealthAgeSeconds = ([DateTimeOffset]::UtcNow - $daemonHealthObservedAtUtc.ToUniversalTime()).TotalSeconds
if ($daemonHealthAgeSeconds -gt $MaxWitnessAgeSeconds) {
  Fail-IdunnHealth -State "failed" -Message "Nightwing Eve browser reference daemon-health record is stale ($([math]::Round($daemonHealthAgeSeconds))s old)."
}

Write-Host "Nightwing Eve browser reference is active, and its daemon-owned CultCache witness now certifies manifest, boundary, and daemon-health truth directly."

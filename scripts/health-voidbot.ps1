param(
  [string] $VoidBotRoot = "E:\Projects\VoidBot",
  [int] $MaxOrchestratorAgeSeconds = 180,
  [int] $MaxCultMeshStoreAgeSeconds = 180
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Fail-IdunnHealth {
  param(
    [string] $State,
    [string] $Message
  )

  Write-Output "idunn.health.state=$State"
  throw $Message
}

function Read-JsonFile {
  param([Parameter(Mandatory = $true)][string] $Path)

  if (-not (Test-Path -LiteralPath $Path)) {
    return $null
  }

  $raw = Get-Content -LiteralPath $Path -Raw -Encoding UTF8
  if ([string]::IsNullOrWhiteSpace($raw)) {
    return $null
  }

  return $raw | ConvertFrom-Json
}

function Get-JsonPropertyValue {
  param(
    [Parameter(Mandatory = $false)] $Object,
    [Parameter(Mandatory = $true)][string] $Name,
    [Parameter(Mandatory = $false)] $Default = $null
  )

  if ($null -eq $Object) {
    return $Default
  }

  $property = $Object.PSObject.Properties[$Name]
  if ($null -eq $property) {
    return $Default
  }

  return $property.Value
}

function Get-ProcessOrFail {
  param(
    [Parameter(Mandatory = $true)][string] $Role,
    [Parameter(Mandatory = $true)][int] $ProcessId
  )

  $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
  if ($null -eq $process) {
    Fail-IdunnHealth -State "failed" -Message "VoidBot $Role process PID $ProcessId is not running."
  }

  return $process
}

function Assert-StoreContains {
  param(
    [Parameter(Mandatory = $true)][string] $StorePath,
    [Parameter(Mandatory = $true)][string[]] $Tokens
  )

  $rg = (Get-Command rg.exe -ErrorAction SilentlyContinue).Source
  if ([string]::IsNullOrWhiteSpace($rg)) {
    Fail-IdunnHealth -State "degraded" -Message "ripgrep is required to inspect the VoidBot CultMesh witness store."
  }

  foreach ($token in $Tokens) {
    & $rg -a -q --fixed-strings -- $token $StorePath
    if ($LASTEXITCODE -ne 0) {
      Fail-IdunnHealth -State "degraded" -Message "VoidBot CultMesh witness store is missing '$token'."
    }
  }
}

$runtimePath = Join-Path $VoidBotRoot ".voidbot\status\runtime-stack.json"
$orchestratorPath = Join-Path $VoidBotRoot ".voidbot\status\gamecult-orchestrator.json"
$cultMeshStorePath = Join-Path $VoidBotRoot ".voidbot\status\cultmesh\voidbot-swarm-state.cc"

$runtime = Read-JsonFile -Path $runtimePath
if ($null -eq $runtime) {
  Fail-IdunnHealth -State "failed" -Message "VoidBot runtime state is missing at $runtimePath."
}

if (-not [bool](Get-JsonPropertyValue -Object $runtime -Name "ready" -Default $false)) {
  Fail-IdunnHealth -State "failed" -Message "VoidBot runtime stack reports ready=false."
}

$runtimeStage = [string](Get-JsonPropertyValue -Object $runtime -Name "stage" -Default "unknown")
if ($runtimeStage -ne "ready") {
  Fail-IdunnHealth -State "failed" -Message "VoidBot runtime stack stage is '$runtimeStage', expected 'ready'."
}

$bot = Get-JsonPropertyValue -Object $runtime -Name "bot"
$worker = Get-JsonPropertyValue -Object $runtime -Name "worker"
$botPid = if ($null -ne $bot) { [int](Get-JsonPropertyValue -Object $bot -Name "pid" -Default 0) } else { 0 }
$workerPid = if ($null -ne $worker) { [int](Get-JsonPropertyValue -Object $worker -Name "pid" -Default 0) } else { 0 }
if ($botPid -le 0) {
  Fail-IdunnHealth -State "failed" -Message "VoidBot runtime state does not contain a live bot PID."
}
if ($workerPid -le 0) {
  Fail-IdunnHealth -State "failed" -Message "VoidBot runtime state does not contain a live worker PID."
}

$botProcess = Get-ProcessOrFail -Role "bot" -ProcessId $botPid
$workerProcess = Get-ProcessOrFail -Role "worker" -ProcessId $workerPid

$orchestrator = Read-JsonFile -Path $orchestratorPath
if ($null -eq $orchestrator -or $null -eq $orchestrator.organs) {
  Fail-IdunnHealth -State "failed" -Message "VoidBot orchestrator state is missing at $orchestratorPath."
}

$surfaceState = $orchestrator.organs.PSObject.Properties["voidbot-swarm-surface"]
if ($null -eq $surfaceState) {
  Fail-IdunnHealth -State "failed" -Message "VoidBot orchestrator does not report the voidbot-swarm-surface organ."
}
$surfaceState = $surfaceState.Value

if ($surfaceState.lastStatus -ne "ok") {
  Fail-IdunnHealth -State "degraded" -Message "VoidBot swarm surface organ lastStatus is '$($surfaceState.lastStatus)', expected 'ok'."
}

if ([string]::IsNullOrWhiteSpace($surfaceState.lastFinishedAt)) {
  Fail-IdunnHealth -State "failed" -Message "VoidBot swarm surface organ has no lastFinishedAt timestamp."
}

$surfaceFinishedAt = [DateTimeOffset]::Parse($surfaceState.lastFinishedAt).ToUniversalTime()
$surfaceAgeSeconds = ([DateTimeOffset]::UtcNow - $surfaceFinishedAt).TotalSeconds
if ($surfaceAgeSeconds -gt $MaxOrchestratorAgeSeconds) {
  Fail-IdunnHealth -State "failed" -Message "VoidBot swarm surface organ is stale ($([math]::Round($surfaceAgeSeconds))s old)."
}

$orchestratorFile = Get-Item -LiteralPath $orchestratorPath -ErrorAction SilentlyContinue
if ($null -eq $orchestratorFile) {
  Fail-IdunnHealth -State "failed" -Message "VoidBot orchestrator state file is missing at $orchestratorPath."
}
$orchestratorAgeSeconds = ([DateTimeOffset]::UtcNow - [DateTimeOffset]$orchestratorFile.LastWriteTimeUtc).TotalSeconds
if ($orchestratorAgeSeconds -gt $MaxOrchestratorAgeSeconds) {
  Fail-IdunnHealth -State "failed" -Message "VoidBot orchestrator state file is stale ($([math]::Round($orchestratorAgeSeconds))s old)."
}

$cultMeshStore = Get-Item -LiteralPath $cultMeshStorePath -ErrorAction SilentlyContinue
if ($null -eq $cultMeshStore) {
  Fail-IdunnHealth -State "failed" -Message "VoidBot CultMesh witness store is missing at $cultMeshStorePath."
}
if ($cultMeshStore.Length -le 0) {
  Fail-IdunnHealth -State "failed" -Message "VoidBot CultMesh witness store at $cultMeshStorePath is empty."
}

$cultMeshStoreAgeSeconds = ([DateTimeOffset]::UtcNow - [DateTimeOffset]$cultMeshStore.LastWriteTimeUtc).TotalSeconds
if ($cultMeshStoreAgeSeconds -gt $MaxCultMeshStoreAgeSeconds) {
  Fail-IdunnHealth -State "failed" -Message "VoidBot CultMesh witness store is stale ($([math]::Round($cultMeshStoreAgeSeconds))s old)."
}

Assert-StoreContains -StorePath $cultMeshStorePath -Tokens @(
  "voidbot.provider_advertisement_catalog.v0",
  "idunn.command_boundary.v1",
  "idunn.daemon_transport_profile.v1",
  "voidbot.providers",
  "voidbot.discord",
  "voidbot.reddit",
  "voidbot.archive",
  "voidbot.source",
  "voidbot.repo_face",
  "voidbot.swarm",
  "command-boundary:voidbot",
  "transport:voidbot"
)

Write-Host "VoidBot runtime is ready, bot PID $($botProcess.Id) and worker PID $($workerProcess.Id) are live, and the daemon-owned CultMesh witness store is fresh."

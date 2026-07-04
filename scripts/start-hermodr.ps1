param(
  [string] $HostAddress = $(if ($env:HERMODR_BIND_HOST) { $env:HERMODR_BIND_HOST } else { "127.0.0.1" }),
  [int] $Port = $(if ($env:HERMODR_PORT) { [int]$env:HERMODR_PORT } else { 8798 }),
  [string] $OdinCultMeshUri = $(if ($env:HERMODR_ODIN_CULTMESH_URI) { $env:HERMODR_ODIN_CULTMESH_URI } elseif ($env:ODIN_CULTMESH_URI) { $env:ODIN_CULTMESH_URI } else { "cultmesh://odin/rendezvous/provider-catalog" }),
  [string] $StateDir = $(Join-Path $PSScriptRoot "..\scratch\Hermodr"),
  [switch] $NoWindow
)

$ErrorActionPreference = "Stop"

foreach ($name in @(
  "HERMODR_ODIN_HTTP_BASE",
  "HERMODR_ODIN_WS_BASE",
  "HERMODR_ODIN_STATE_PATH",
  "HERMODR_SLEIPNIR_COMMAND_RUDP",
  "HERMODR_SLEIPNIR_COMMAND_ENDPOINT",
  "HERMODR_MUNINN_RUDP",
  "HERMODR_AETHERIA_COMMAND_RUDP",
  "HERMODR_ODIN_CULTMESH_STORE",
  "ODIN_CULTMESH_STORE"
)) {
  if (-not [string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($name))) {
    throw "$name was removed. Hermodr lowers Odin/CultMesh state for browsers and may only publish typed CultMesh command documents."
  }
}
if (-not [string]::IsNullOrWhiteSpace($env:HERMODR_COMMAND_STORE)) {
  throw "HERMODR_COMMAND_STORE was removed. Hermodr publishes typed commands through provider-advertised CultMesh routes and records visible witnesses in Odin's CultMesh cache."
}

$node = "node"
$Hermodr = Join-Path $PSScriptRoot "..\src\hermodr-daemon.cjs"
$arguments = @(
  $Hermodr,
  "--host", $HostAddress,
  "--port", $Port,
  "--odin-cultmesh-uri", $OdinCultMeshUri
)

if ($NoWindow) {
  New-Item -ItemType Directory -Force -Path $StateDir | Out-Null
  $proc = Start-Process -FilePath $node -ArgumentList $arguments -WindowStyle Hidden -PassThru
  $proc.Id | Set-Content -Encoding ASCII -LiteralPath (Join-Path $StateDir "hermodr.pid")
  Write-Host "Started Hermodr browser lowering at http://${HostAddress}:${Port}/"
  return
}

& $node @arguments

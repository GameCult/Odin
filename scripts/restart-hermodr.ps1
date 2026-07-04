param(
  [string] $HostAddress = $(if ($env:HERMODR_BIND_HOST) { $env:HERMODR_BIND_HOST } else { "127.0.0.1" }),
  [int] $Port = $(if ($env:HERMODR_PORT) { [int]$env:HERMODR_PORT } else { 8798 }),
  [string] $OdinCultMeshUri = $(if ($env:HERMODR_ODIN_CULTMESH_URI) { $env:HERMODR_ODIN_CULTMESH_URI } elseif ($env:ODIN_CULTMESH_URI) { $env:ODIN_CULTMESH_URI } else { "cultmesh://odin/rendezvous/provider-catalog" })
)

$ErrorActionPreference = "Stop"

$stop = Join-Path $PSScriptRoot "stop-hermodr.ps1"
$start = Join-Path $PSScriptRoot "start-hermodr.ps1"

if (Test-Path -LiteralPath $stop) {
  & $stop
}

& $start -HostAddress $HostAddress -Port $Port -OdinCultMeshUri $OdinCultMeshUri -NoWindow

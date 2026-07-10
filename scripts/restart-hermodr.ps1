param(
  [string] $HostAddress = $(if ($env:HERMODR_BIND_HOST) { $env:HERMODR_BIND_HOST } else { "127.0.0.1" }),
  [int] $Port = $(if ($env:HERMODR_PORT) { [int]$env:HERMODR_PORT } else { 8798 }),
  [string] $OdinCultMeshUri = $(if ($env:HERMODR_ODIN_CULTMESH_URI) { $env:HERMODR_ODIN_CULTMESH_URI } elseif ($env:ODIN_CULTMESH_URI) { $env:ODIN_CULTMESH_URI } else { "cultmesh://odin/rendezvous/provider-catalog" }),
  [string] $IdunnRudpHealth = $(if ($env:HERMODR_IDUNN_RUDP_HEALTH) { $env:HERMODR_IDUNN_RUDP_HEALTH } else { $env:IDUNN_RUDP_HEALTH }),
  [string] $IdunnDaemon = "hermodr",
  [string] $IdunnHealthContract = "hermodr.cultnet-rudp-browser-lowering-health"
)

$ErrorActionPreference = "Stop"

if ($env:IDUNN_ACTUATOR -ne "1" -or $env:IDUNN_COMMAND_AUTHORITY -ne "idunn-daemon") {
  throw "restart-hermodr.ps1 is an Idunn actuator body. Redeploy by poking Idunn; direct service restart is not an owned path."
}

$stop = Join-Path $PSScriptRoot "stop-hermodr.ps1"
$start = Join-Path $PSScriptRoot "start-hermodr.ps1"

if (Test-Path -LiteralPath $stop) {
  & $stop
}

& $start -HostAddress $HostAddress -Port $Port -OdinCultMeshUri $OdinCultMeshUri -IdunnRudpHealth $IdunnRudpHealth -IdunnDaemon $IdunnDaemon -IdunnHealthContract $IdunnHealthContract -NoWindow

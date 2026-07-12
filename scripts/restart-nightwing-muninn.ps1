param(
  [string] $SshTarget = "nightwing",
  [string] $MuninnExe = "/home/metacrat/.local/bin/muninn",
  [string] $StorePath = "/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc",
  [string] $LogRoot = "/home/metacrat/.local/state/gamecult/muninn",
  [string[]] $MoveState = @(),
  [switch] $DiscoverMoveState = $true,
  [switch] $ClaimUsbMoves = $true,
  [string] $MoveEvidenceStream = "muninn:nightwing:move-evidence",
  [string[]] $MoveMarkerCamera = @(
    "nightwing-eye-0=/dev/video2",
    "nightwing-eye-1=/dev/video3"
  ),
  [int] $MoveTrackerExposureMilli = 100,
  [int] $IntervalSeconds = 15,
  [string] $IdunnRudpHealth = $env:IDUNN_RUDP_HEALTH,
  [string] $IdunnDaemon = "nightwing-muninn",
  [string] $IdunnHealthContract = "muninn.cultnet-rudp-remote-telemetry-and-move-hid",
  [string] $OdinCultMeshUri = $(if ($env:ODIN_CULTMESH_URI) { $env:ODIN_CULTMESH_URI } else { "cultmesh://odin/rendezvous/provider-catalog" }),
  [string] $OdinCultMeshRudpEndpoint = $(if ($env:CULTMESH_URI_ODIN_RUDP) { $env:CULTMESH_URI_ODIN_RUDP } else { "10.77.0.2:17871" }),
  [string] $HidControllerRudpBind = "0.0.0.0:17888",
  [string] $HidControllerRudpAdvertise = $(if ($env:MUNINN_HID_CONTROLLER_RUDP_ADVERTISE) { $env:MUNINN_HID_CONTROLLER_RUDP_ADVERTISE } else { "10.77.0.3:17888" })
)

$ErrorActionPreference = "Stop"

if ($env:IDUNN_ACTUATOR -ne "1" -or $env:IDUNN_COMMAND_AUTHORITY -ne "idunn-daemon") {
  throw "restart-nightwing-muninn.ps1 is an Idunn actuator body. Redeploy by poking Idunn; direct service restart is not an owned path."
}

if ([string]::IsNullOrWhiteSpace($IdunnRudpHealth)) {
  throw "Idunn RUDP health endpoint must be supplied by -IdunnRudpHealth or IDUNN_RUDP_HEALTH; no Starfire LAN default is allowed."
}
if ($IdunnRudpHealth -match '^(127\.0\.0\.1|localhost):(\d+)$') {
  $IdunnRudpHealth = "10.77.0.2:$($Matches[2])"
}
if (-not [string]::IsNullOrWhiteSpace($HidControllerRudpBind) -and [string]::IsNullOrWhiteSpace($HidControllerRudpAdvertise)) {
  throw "HID controller RUDP advertise endpoint must be supplied by -HidControllerRudpAdvertise or MUNINN_HID_CONTROLLER_RUDP_ADVERTISE when HID RUDP bind is enabled; no Nightwing LAN default is allowed."
}

function Quote-ShSingle([string] $Value) {
  return "'" + ($Value -replace "'", "'\''") + "'"
}

function Set-AsciiFile {
  param(
    [Parameter(Mandatory = $true)] [string] $Path,
    [Parameter(Mandatory = $true)] [string] $Content
  )

  [System.IO.File]::WriteAllText($Path, ($Content -replace "`r?`n", "`n"), [System.Text.Encoding]::ASCII)
}

function Invoke-UploadedShell {
  param(
    [Parameter(Mandatory = $true)] [string] $SshTarget,
    [Parameter(Mandatory = $true)] [string] $RemoteScriptContent
  )

  $uploadId = [guid]::NewGuid().ToString("N")
  $localTempRoot = Join-Path $env:TEMP "odin-nightwing-muninn-restart-$uploadId"
  $localRemoteScript = Join-Path $localTempRoot "restart-nightwing-muninn-$uploadId.sh"
  $localSftpBatch = Join-Path $localTempRoot "restart-nightwing-muninn-$uploadId.sftp"
  $remoteScriptPath = "/tmp/restart-nightwing-muninn-$uploadId.sh"

  try {
    New-Item -ItemType Directory -Force -Path $localTempRoot | Out-Null
    Set-AsciiFile -Path $localRemoteScript -Content $RemoteScriptContent
    Set-AsciiFile -Path $localSftpBatch -Content ('put "{0}" "{1}"' -f $localRemoteScript, $remoteScriptPath)

    & sftp.exe -b $localSftpBatch $SshTarget
    if ($LASTEXITCODE -ne 0) {
      exit $LASTEXITCODE
    }

    & ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $SshTarget "chmod +x '$remoteScriptPath' && bash '$remoteScriptPath'"
    $remoteExit = $LASTEXITCODE
    & ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $SshTarget "rm -f '$remoteScriptPath'"
    exit $remoteExit
  } finally {
    Remove-Item -LiteralPath $localTempRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}

$moveStateSpecs = @($MoveState | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
if ($ClaimUsbMoves.IsPresent -and -not $DiscoverMoveState.IsPresent -and $moveStateSpecs.Count -eq 0) {
  throw "-ClaimUsbMoves requires -DiscoverMoveState or at least one explicit -MoveState value."
}
if ($DiscoverMoveState.IsPresent) {
  $claimScript = Join-Path $PSScriptRoot "claim-nightwing-usb-moves.ps1"
  if ($ClaimUsbMoves.IsPresent) {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $claimScript -SshTarget $SshTarget
  }
  $sourceScript = Join-Path $PSScriptRoot "get-nightwing-move-state-sources.ps1"
  $moveStateSpecs = @(& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $sourceScript -SshTarget $SshTarget)
  if ($moveStateSpecs.Count -eq 0) {
    throw "Nightwing Move runtime was explicitly requested, but no live Move state sources were discovered."
  }
}

$moveStateSetLines = ($moveStateSpecs | ForEach-Object {
  "set -- ""`$@"" --move-state $(Quote-ShSingle $_)"
}) -join "`n"
$moveRuntimeSetLines = @()
if ($moveStateSpecs.Count -gt 0) {
  $moveRuntimeSetLines += $moveStateSetLines
  $moveRuntimeSetLines += "set -- ""`$@"" --move-evidence-stream $(Quote-ShSingle $MoveEvidenceStream)"
  if ($MoveMarkerCamera.Count -gt 0) {
    foreach ($camera in $MoveMarkerCamera) {
      $moveRuntimeSetLines += "set -- ""`$@"" --move-marker-camera $(Quote-ShSingle $camera)"
    }
    $moveRuntimeSetLines += "set -- ""`$@"" --move-psmoveapi-tracker"
    $moveRuntimeSetLines += "set -- ""`$@"" --move-tracker-exposure-milli $(Quote-ShSingle ([string]$MoveTrackerExposureMilli))"
  }
}
$moveRuntimeSetBlock = ($moveRuntimeSetLines -join "`n")
$odinCultMeshUriSetBlock = ""
if (-not [string]::IsNullOrWhiteSpace($OdinCultMeshUri)) {
  $odinCultMeshUriSetBlock = "set -- ""`$@"" --odin-cultmesh-uri $(Quote-ShSingle $OdinCultMeshUri)"
}
$hidControllerRudpSetLines = @()
if (-not [string]::IsNullOrWhiteSpace($HidControllerRudpBind)) {
  $hidControllerRudpSetLines += "set -- ""`$@"" --hid-controller-rudp-bind $(Quote-ShSingle $HidControllerRudpBind)"
}
if (-not [string]::IsNullOrWhiteSpace($HidControllerRudpAdvertise)) {
  $hidControllerRudpSetLines += "set -- ""`$@"" --hid-controller-rudp-advertise $(Quote-ShSingle $HidControllerRudpAdvertise)"
}
$hidControllerRudpSetBlock = ($hidControllerRudpSetLines -join "`n")

$remoteScript = @"
set -eu
mkdir -p '$LogRoot'
if [ ! -x '$MuninnExe' ]; then
  echo 'Muninn executable not found at $MuninnExe' >&2
  exit 1
fi
if [ -f '$LogRoot/muninn.pid' ] && kill -0 "`$(cat '$LogRoot/muninn.pid')" 2>/dev/null; then
  kill "`$(cat '$LogRoot/muninn.pid')" 2>/dev/null || true
  sleep 1
fi
for pid in `$(pgrep -f '[m]uninn serve .*--host nightwing' 2>/dev/null || true); do
  kill "`$pid" 2>/dev/null || true
done
set -- serve \
  --store '$StorePath' \
  --log-root '$LogRoot' \
  --host nightwing
$moveRuntimeSetBlock
set -- "`$@" \
  --interval-seconds '$IntervalSeconds' \
  --idunn-rudp-health '$IdunnRudpHealth' \
  --idunn-daemon '$IdunnDaemon' \
  --idunn-health-contract '$IdunnHealthContract'
$odinCultMeshUriSetBlock
$hidControllerRudpSetBlock
CULTMESH_URI_ODIN_RUDP='$OdinCultMeshRudpEndpoint' nohup '$MuninnExe' "`$@" \
  > '$LogRoot/muninn-serve.out.log' \
  2> '$LogRoot/muninn-serve.err.log' \
  < /dev/null &
echo `$! > '$LogRoot/muninn.pid'
sleep 1
muninn_pid="`$(cat '$LogRoot/muninn.pid')"
kill -0 "`$muninn_pid" 2>/dev/null
muninn_cmdline="`$(tr '\0' ' ' < "/proc/`$muninn_pid/cmdline")"
if ! printf '%s\n' "`$muninn_cmdline" | grep -F -- '--host nightwing' >/dev/null 2>&1 ||
   ! printf '%s\n' "`$muninn_cmdline" | grep -F -- '--idunn-rudp-health $IdunnRudpHealth' >/dev/null 2>&1; then
  echo 'Nightwing Muninn serve command line is missing Idunn RUDP health arguments' >&2
  exit 1
fi
"@

Invoke-UploadedShell -SshTarget $SshTarget -RemoteScriptContent $remoteScript

param(
  [string] $RavenHost = "raven",
  [string] $LocalViliRoot = "E:\Projects\Vili",
  [string] $LocalCultLibRoot = "E:\Projects\CultLib",
  [string] $RemoteViliRoot = "E:\Projects\Vili",
  [int] $Port = 8824,
  [string] $HostName = "0.0.0.0",
  [string] $StateRoot = "E:\Projects\Vili\.vili",
  [string] $IdunnRudpHealth = "10.77.0.2:17870",
  [string] $IdunnDaemon = "vili",
  [string] $IdunnHealthContract = "vili.cultnet-rudp-animation-health",
  [int] $IdunnHealthIntervalSeconds = 15
)

$ErrorActionPreference = "Stop"

$localFiles = @(
  (Join-Path $LocalViliRoot "package.json"),
  (Join-Path $LocalViliRoot "package-lock.json"),
  (Join-Path $LocalViliRoot "scripts\health-vili-daemon.cmd"),
  (Join-Path $LocalViliRoot "scripts\idunn-rudp.cjs"),
  (Join-Path $LocalViliRoot "scripts\install-vili-task.ps1"),
  (Join-Path $LocalViliRoot "scripts\restart-vili-daemon.cmd"),
  (Join-Path $LocalViliRoot "scripts\restart-vili-daemon.ps1"),
  (Join-Path $LocalViliRoot "scripts\run-hidden-powershell.vbs"),
  (Join-Path $LocalViliRoot "scripts\run-vili-daemon.ps1"),
  (Join-Path $LocalViliRoot "scripts\start-vili-daemon.ps1"),
  (Join-Path $LocalViliRoot "scripts\vili-daemon.mjs"),
  (Join-Path $LocalCultLibRoot "packages\cultnet-ts"),
  (Join-Path $LocalCultLibRoot "packages\cultcache-ts"),
  (Join-Path $LocalCultLibRoot "node_modules\ajv"),
  (Join-Path $LocalCultLibRoot "node_modules\fast-deep-equal"),
  (Join-Path $LocalCultLibRoot "node_modules\fast-uri"),
  (Join-Path $LocalCultLibRoot "node_modules\json-schema-traverse"),
  (Join-Path $LocalCultLibRoot "node_modules\require-from-string"),
  (Join-Path $LocalViliRoot "node_modules\@msgpack"),
  (Join-Path $LocalViliRoot "node_modules\ws")
)

foreach ($path in $localFiles) {
  if (-not (Test-Path -LiteralPath $path)) {
    throw "Required Vili runtime path not found: $path"
  }
}

$syncId = [guid]::NewGuid().ToString("N")
$remoteSyncRootPs = "$RemoteViliRoot\.odin-vili-sync-$syncId"
$remoteSyncRootSftp = ($remoteSyncRootPs -replace "\\", "/")
$remoteTempPs = "C:\Windows\Temp\odin-raven-vili-restart-$syncId.ps1"
$remoteTempSftp = ($remoteTempPs -replace "\\", "/")
$remoteBundlePs = "C:\Windows\Temp\odin-raven-vili-bundle-$syncId.zip"
$remoteBundleSftp = ($remoteBundlePs -replace "\\", "/")
$localRemoteScript = Join-Path $env:TEMP "odin-raven-vili-restart-$syncId.ps1"
$localSftpBatch = Join-Path $env:TEMP "odin-raven-vili-restart-$syncId.sftp"
$localBundleRoot = Join-Path $env:TEMP "odin-raven-vili-bundle-$syncId"
$localBundleScripts = Join-Path $localBundleRoot "scripts"
$localBundleNodeModules = Join-Path $localBundleRoot "node_modules"
$localBundleZip = Join-Path $env:TEMP "odin-raven-vili-bundle-$syncId.zip"

$remoteScript = @"
`$ErrorActionPreference = "Stop"
`$ProgressPreference = "SilentlyContinue"

`$remoteRoot = "$RemoteViliRoot"
`$syncRoot = "$remoteSyncRootPs"
`$scriptsRoot = Join-Path `$remoteRoot "scripts"
`$stateRoot = "$StateRoot"
`$nodeModulesRoot = Join-Path `$remoteRoot "node_modules"
`$pidPath = Join-Path `$stateRoot "vili-daemon.pid"

try {
  Expand-Archive -LiteralPath "$remoteBundlePs" -DestinationPath `$syncRoot -Force
  if (-not (Test-Path -LiteralPath `$syncRoot)) {
    throw "Uploaded sync root not found at `$syncRoot"
  }

  if (Test-Path -LiteralPath `$pidPath) {
    `$existingPid = (Get-Content -LiteralPath `$pidPath -Raw -ErrorAction SilentlyContinue).Trim()
    if (`$existingPid -match '^\d+$') {
      `$process = Get-Process -Id ([int] `$existingPid) -ErrorAction SilentlyContinue
      if (`$null -ne `$process) {
        Stop-Process -Id `$process.Id -Force
        Start-Sleep -Seconds 1
      }
    }
    Remove-Item -LiteralPath `$pidPath -Force -ErrorAction SilentlyContinue
  }

  New-Item -ItemType Directory -Force -Path `$scriptsRoot | Out-Null
  New-Item -ItemType Directory -Force -Path `$stateRoot | Out-Null

  Copy-Item -LiteralPath (Join-Path `$syncRoot "package.json") -Destination (Join-Path `$remoteRoot "package.json") -Force
  Copy-Item -LiteralPath (Join-Path `$syncRoot "package-lock.json") -Destination (Join-Path `$remoteRoot "package-lock.json") -Force

  `$scriptFiles = @(
    "health-vili-daemon.cmd",
    "idunn-rudp.cjs",
    "install-vili-task.ps1",
    "restart-vili-daemon.cmd",
    "restart-vili-daemon.ps1",
    "run-hidden-powershell.vbs",
    "run-vili-daemon.ps1",
    "start-vili-daemon.ps1",
    "vili-daemon.mjs"
  )

  foreach (`$name in `$scriptFiles) {
    Copy-Item -LiteralPath (Join-Path `$syncRoot "scripts\`$name") -Destination (Join-Path `$scriptsRoot `$name) -Force
  }

  if (Test-Path -LiteralPath `$nodeModulesRoot) {
    Remove-Item -LiteralPath `$nodeModulesRoot -Recurse -Force
  }
  Move-Item -LiteralPath (Join-Path `$syncRoot "node_modules") -Destination `$nodeModulesRoot -Force

  & powershell.exe @(
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy", "Bypass",
    "-File", (Join-Path `$scriptsRoot "install-vili-task.ps1"),
    "-TaskName", "GameCult\Vili",
    "-Port", "$Port",
    "-HostName", "$HostName",
    "-StateRoot", "$StateRoot",
    "-IdunnRudpHealth", "$IdunnRudpHealth",
    "-IdunnDaemon", "$IdunnDaemon",
    "-IdunnHealthContract", "$IdunnHealthContract",
    "-IdunnHealthIntervalSeconds", "$IdunnHealthIntervalSeconds"
  )

  & powershell.exe @(
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy", "Bypass",
    "-File", (Join-Path `$scriptsRoot "restart-vili-daemon.ps1"),
    "-Port", "$Port",
    "-HostName", "$HostName",
    "-StateRoot", "$StateRoot",
    "-IdunnRudpHealth", "$IdunnRudpHealth",
    "-IdunnDaemon", "$IdunnDaemon",
    "-IdunnHealthContract", "$IdunnHealthContract",
    "-IdunnHealthIntervalSeconds", "$IdunnHealthIntervalSeconds"
  )

  `$task = Get-ScheduledTask -TaskName "Vili" -TaskPath "\GameCult\" -ErrorAction Stop
  `$action = @(`$task.Actions)[0]
  if (`$action.Execute -notmatch '(^|\\)wscript\.exe$') {
    throw "GameCult\Vili action executes `$(`$action.Execute), expected wscript.exe"
  }
  if (`$action.Arguments -notlike "*run-hidden-powershell.vbs*") {
    throw "GameCult\Vili action arguments do not reference run-hidden-powershell.vbs"
  }
  `$taskHealthChecks = @(
    (`$action.Arguments -like "*-IdunnRudpHealth*"),
    (`$action.Arguments -like "*$IdunnRudpHealth*"),
    (`$action.Arguments -like "*-IdunnDaemon*"),
    (`$action.Arguments -like "*$IdunnDaemon*"),
    (`$action.Arguments -like "*-IdunnHealthContract*"),
    (`$action.Arguments -like "*$IdunnHealthContract*")
  )
  if (`$taskHealthChecks -contains `$false) {
    throw "GameCult\Vili action arguments do not carry the Idunn RUDP health contract."
  }

  `$process = Get-CimInstance Win32_Process |
    Where-Object { `$_.CommandLine -like '*vili-daemon.mjs*' } |
    Select-Object -First 1
  if (`$null -eq `$process) {
    throw "Vili process is not running after restart."
  }
  `$processHealthChecks = @(
    (`$process.CommandLine -like "*--idunn-rudp-health*"),
    (`$process.CommandLine -like "*$IdunnRudpHealth*"),
    (`$process.CommandLine -like "*--idunn-daemon*"),
    (`$process.CommandLine -like "*$IdunnDaemon*"),
    (`$process.CommandLine -like "*--idunn-health-contract*"),
    (`$process.CommandLine -like "*$IdunnHealthContract*")
  )
  if (`$processHealthChecks -contains `$false) {
    throw "Vili process command line does not include the Idunn RUDP health arguments."
  }

  `$statePath = Join-Path `$stateRoot "operator-state.json"
  `$deadline = (Get-Date).AddSeconds(30)
  `$state = `$null
  while ((Get-Date) -lt `$deadline) {
    if (Test-Path -LiteralPath `$statePath) {
      try {
        `$state = Get-Content -LiteralPath `$statePath -Raw | ConvertFrom-Json
        if (`$null -ne `$state.idunnRudpHealth -and `$state.idunnRudpHealth.status -eq "published") {
          break
        }
      } catch {
      }
    }
    Start-Sleep -Seconds 1
  }
  if (`$null -eq `$state -or `$null -eq `$state.idunnRudpHealth) {
    throw "Vili operator-state.json did not publish idunnRudpHealth after restart."
  }
  if (`$state.idunnRudpHealth.status -ne "published") {
    throw "Vili idunnRudpHealth status is `$(`$state.idunnRudpHealth.status): `$(`$state.idunnRudpHealth.error)"
  }

  Write-Host "Vili Raven runtime repaired and restarted with Idunn RUDP health."
  Write-Host "ProcessId=`$(`$process.ProcessId)"
  Write-Host "TaskArguments=`$(`$action.Arguments)"
  Write-Host "HealthStatus=`$(`$state.idunnRudpHealth.status)"
  Write-Host "HealthContract=`$(`$state.idunnRudpHealth.contract)"
} finally {
  Remove-Item -LiteralPath `$syncRoot -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath "$remoteBundlePs" -Force -ErrorAction SilentlyContinue
}
"@

$batchLines = @(
  "put ""$localBundleZip"" ""$remoteBundleSftp""",
  "put ""$localRemoteScript"" ""$remoteTempSftp"""
)

try {
  $moduleCopies = @(
    @{ Source = (Join-Path $LocalCultLibRoot "packages\cultnet-ts"); Destination = (Join-Path $localBundleNodeModules "cultnet-ts") },
    @{ Source = (Join-Path $LocalCultLibRoot "packages\cultcache-ts"); Destination = (Join-Path $localBundleNodeModules "cultcache-ts") },
    @{ Source = (Join-Path $LocalCultLibRoot "node_modules\ajv"); Destination = (Join-Path $localBundleNodeModules "ajv") },
    @{ Source = (Join-Path $LocalCultLibRoot "node_modules\fast-deep-equal"); Destination = (Join-Path $localBundleNodeModules "fast-deep-equal") },
    @{ Source = (Join-Path $LocalCultLibRoot "node_modules\fast-uri"); Destination = (Join-Path $localBundleNodeModules "fast-uri") },
    @{ Source = (Join-Path $LocalCultLibRoot "node_modules\json-schema-traverse"); Destination = (Join-Path $localBundleNodeModules "json-schema-traverse") },
    @{ Source = (Join-Path $LocalCultLibRoot "node_modules\require-from-string"); Destination = (Join-Path $localBundleNodeModules "require-from-string") },
    @{ Source = (Join-Path $LocalViliRoot "node_modules\@msgpack"); Destination = (Join-Path $localBundleNodeModules "@msgpack") },
    @{ Source = (Join-Path $LocalViliRoot "node_modules\ws"); Destination = (Join-Path $localBundleNodeModules "ws") }
  )
  New-Item -ItemType Directory -Force -Path $localBundleScripts, $localBundleNodeModules | Out-Null
  Copy-Item -LiteralPath (Join-Path $LocalViliRoot "package.json") -Destination (Join-Path $localBundleRoot "package.json") -Force
  Copy-Item -LiteralPath (Join-Path $LocalViliRoot "package-lock.json") -Destination (Join-Path $localBundleRoot "package-lock.json") -Force
  $scriptCopies = @(
    "health-vili-daemon.cmd",
    "idunn-rudp.cjs",
    "install-vili-task.ps1",
    "restart-vili-daemon.cmd",
    "restart-vili-daemon.ps1",
    "run-hidden-powershell.vbs",
    "run-vili-daemon.ps1",
    "start-vili-daemon.ps1",
    "vili-daemon.mjs"
  )
  foreach ($scriptName in $scriptCopies) {
    Copy-Item -LiteralPath (Join-Path $LocalViliRoot "scripts\$scriptName") -Destination (Join-Path $localBundleScripts $scriptName) -Force
  }
  foreach ($copy in $moduleCopies) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $copy.Destination) | Out-Null
    Copy-Item -LiteralPath $copy.Source -Destination $copy.Destination -Recurse -Force
  }
  if (Test-Path -LiteralPath $localBundleZip) {
    Remove-Item -LiteralPath $localBundleZip -Force
  }
  Compress-Archive -Path (Join-Path $localBundleRoot "*") -DestinationPath $localBundleZip -CompressionLevel Optimal
  Set-Content -LiteralPath $localRemoteScript -Encoding ASCII -Value $remoteScript
  Set-Content -LiteralPath $localSftpBatch -Encoding ASCII -Value ($batchLines -join "`r`n")
  & sftp.exe -b $localSftpBatch $RavenHost
  if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
  }

  $remoteRunner = @"
`$ErrorActionPreference = "Stop"
try {
  & "$remoteTempPs"
  `$code = `$LASTEXITCODE
} finally {
  Remove-Item -LiteralPath "$remoteTempPs" -Force -ErrorAction SilentlyContinue
}
exit `$code
"@
  $encodedRunner = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($remoteRunner))
  & ssh.exe -o BatchMode=yes -o ConnectTimeout=10 $RavenHost "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand $encodedRunner"
} finally {
  Remove-Item -LiteralPath $localRemoteScript, $localSftpBatch -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $localBundleZip -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $localBundleRoot -Recurse -Force -ErrorAction SilentlyContinue
}

$restartExit = $LASTEXITCODE
if ($restartExit -eq 0) {
  Start-Sleep -Seconds 2
  try {
    & curl.exe -fsS --max-time 15 "http://10.77.0.4:$Port/operator-state" | Out-Null
  } catch {
  }
}
exit $restartExit

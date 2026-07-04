$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "idunn-deployment-targets.ps1")

$unknown = @($IdunnDeploymentTargets | Where-Object { $_.Status -notin @("enforced", "runtime-enforced", "blocked", "external-owned", "not-runtime") })
$enforcedWithoutDeploy = @($IdunnDeploymentTargets | Where-Object { $_.Status -eq "enforced" -and [string]::IsNullOrWhiteSpace($_.Deploy) })
$enforcedWithoutHealth = @($IdunnDeploymentTargets | Where-Object { $_.Status -in @("enforced", "runtime-enforced") -and $_.Id -ne "idunn" -and [string]::IsNullOrWhiteSpace($_.Health) })
$enforcedWithoutUpstream = @($IdunnDeploymentTargets | Where-Object { $_.Status -eq "enforced" -and ([string]::IsNullOrWhiteSpace($_.UpstreamRemote) -or [string]::IsNullOrWhiteSpace($_.UpstreamBranch)) })
$enforcedWithoutRollout = @($IdunnDeploymentTargets | Where-Object { $_.Status -eq "enforced" -and ([string]::IsNullOrWhiteSpace($_.RolloutStrategy) -or [string]::IsNullOrWhiteSpace($_.StateMigration) -or [string]::IsNullOrWhiteSpace($_.ZeroDowntime)) })
$blockedWithoutReason = @($IdunnDeploymentTargets | Where-Object { $_.Status -in @("blocked", "external-owned") -and [string]::IsNullOrWhiteSpace($_.Reason) })
$deployScriptsWithoutIdunnGuard = @(
  $IdunnDeploymentTargets |
    Where-Object { $_.Status -eq "enforced" -and -not [string]::IsNullOrWhiteSpace($_.Deploy) } |
    Where-Object {
      -not (Test-Path -LiteralPath $_.Deploy) -or
      -not ((Get-Content -Raw -LiteralPath $_.Deploy) -match 'IDUNN_ACTUATOR')
    }
)
$knownActuatorPaths = @{}
foreach ($target in $IdunnDeploymentTargets) {
  foreach ($path in @($target.Deploy, $target.Restart) + @($target.Aliases)) {
    if (-not [string]::IsNullOrWhiteSpace($path)) {
      $knownActuatorPaths[[System.IO.Path]::GetFullPath($path).ToLowerInvariant()] = $target.Id
    }
  }
}
foreach ($path in $IdunnDeploymentSharedActuators) {
  if (-not [string]::IsNullOrWhiteSpace($path)) {
    $knownActuatorPaths[[System.IO.Path]::GetFullPath($path).ToLowerInvariant()] = "shared"
  }
}
$guardedActuatorsWithoutCatalog = @(
  Get-ChildItem -LiteralPath $PSScriptRoot -File |
    Where-Object {
      $_.Name -match '^(deploy|restart)-.*\.(ps1|cmd)$' -and
      (Get-Content -Raw -LiteralPath $_.FullName) -match 'IDUNN_ACTUATOR' -and
      -not $knownActuatorPaths.ContainsKey([System.IO.Path]::GetFullPath($_.FullName).ToLowerInvariant())
    }
)

$issues = @()
if ($unknown.Count) {
  $issues += "unknown status: $($unknown.Id -join ', ')"
}
if ($enforcedWithoutDeploy.Count) {
  $issues += "enforced target missing deploy command: $($enforcedWithoutDeploy.Id -join ', ')"
}
if ($enforcedWithoutHealth.Count) {
  $issues += "enforced target missing health command: $($enforcedWithoutHealth.Id -join ', ')"
}
if ($enforcedWithoutUpstream.Count) {
  $issues += "enforced target missing upstream remote/branch: $($enforcedWithoutUpstream.Id -join ', ')"
}
if ($enforcedWithoutRollout.Count) {
  $issues += "enforced target missing rollout/migration/downtime contract: $($enforcedWithoutRollout.Id -join ', ')"
}
if ($deployScriptsWithoutIdunnGuard.Count) {
  $issues += "enforced deploy script missing Idunn actuator guard: $($deployScriptsWithoutIdunnGuard.Id -join ', ')"
}
if ($guardedActuatorsWithoutCatalog.Count) {
  $issues += "guarded deploy/restart actuator missing Idunn catalog entry: $($guardedActuatorsWithoutCatalog.Name -join ', ')"
}
if ($blockedWithoutReason.Count) {
  $issues += "blocked/external target missing reason: $($blockedWithoutReason.Id -join ', ')"
}

if ($issues.Count) {
  throw "Idunn deployment target catalog is incoherent: $($issues -join '; ')"
}

$enforced = @($IdunnDeploymentTargets | Where-Object { $_.Status -eq "enforced" })
$runtimeEnforced = @($IdunnDeploymentTargets | Where-Object { $_.Status -eq "runtime-enforced" })
$blocked = @($IdunnDeploymentTargets | Where-Object { $_.Status -eq "blocked" })
$external = @($IdunnDeploymentTargets | Where-Object { $_.Status -eq "external-owned" })
Write-Host "Idunn deployment catalog coherent: $($IdunnDeploymentTargets.Count) targets, $($enforced.Count) deploy-enforced with upstream rollout contracts, $($runtimeEnforced.Count) runtime-enforced, $($blocked.Count) blocked, $($external.Count) external-owned, $($knownActuatorPaths.Count) cataloged actuator paths."

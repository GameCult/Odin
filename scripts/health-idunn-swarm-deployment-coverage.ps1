$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "idunn-deployment-targets.ps1")

$unknown = @($IdunnDeploymentTargets | Where-Object { $_.Status -notin @("enforced", "blocked", "external-owned", "not-runtime") })
$enforcedWithoutDeploy = @($IdunnDeploymentTargets | Where-Object { $_.Status -eq "enforced" -and [string]::IsNullOrWhiteSpace($_.Deploy) })
$enforcedWithoutHealth = @($IdunnDeploymentTargets | Where-Object { $_.Status -eq "enforced" -and [string]::IsNullOrWhiteSpace($_.Health) })
$blockedWithoutReason = @($IdunnDeploymentTargets | Where-Object { $_.Status -in @("blocked", "external-owned") -and [string]::IsNullOrWhiteSpace($_.Reason) })

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
if ($blockedWithoutReason.Count) {
  $issues += "blocked/external target missing reason: $($blockedWithoutReason.Id -join ', ')"
}

if ($issues.Count) {
  throw "Idunn deployment target catalog is incoherent: $($issues -join '; ')"
}

$enforced = @($IdunnDeploymentTargets | Where-Object { $_.Status -eq "enforced" })
$blocked = @($IdunnDeploymentTargets | Where-Object { $_.Status -eq "blocked" })
$external = @($IdunnDeploymentTargets | Where-Object { $_.Status -eq "external-owned" })
Write-Host "Idunn deployment catalog coherent: $($IdunnDeploymentTargets.Count) targets, $($enforced.Count) enforced, $($blocked.Count) blocked, $($external.Count) external-owned."

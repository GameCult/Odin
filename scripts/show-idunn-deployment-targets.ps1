$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "idunn-deployment-targets.ps1")

$IdunnDeploymentTargets |
  Sort-Object Status, Id |
  Select-Object Id, Repo, Host, Service, Status, Reason |
  Format-Table -AutoSize -Wrap

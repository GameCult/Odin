param(
  [string] $TaskName = "Idunn Local Keepalive",
  [int] $IntervalSeconds = 30
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$scriptPath = Join-Path $repoRoot "scripts\start-idunn-local.ps1"
$powershell = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
$taskCommand = "`"$powershell`" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$scriptPath`" -IntervalSeconds $IntervalSeconds"

& schtasks.exe /Create /TN $TaskName /SC ONLOGON /TR $taskCommand /F | Out-Null
if ($LASTEXITCODE -ne 0) {
  $startupDir = [Environment]::GetFolderPath("Startup")
  $startupScript = Join-Path $startupDir "$TaskName.cmd"
  $cmd = "@echo off`r`ncd /d `"$repoRoot`"`r`n$taskCommand`r`n"
  [System.IO.File]::WriteAllText($startupScript, $cmd, [System.Text.UTF8Encoding]::new($false))
  Write-Host "Scheduled task install was denied; installed Startup folder launcher instead:"
  Write-Host "  $startupScript"
} else {
  Write-Host "Installed scheduled task: $TaskName"
}

Write-Host "Starting Idunn watchdogs now..."
& $scriptPath -IntervalSeconds $IntervalSeconds

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$voidBotRoot = "E:\Projects\VoidBot"
$checkScript = Join-Path $voidBotRoot "scripts\check-voidbot-operations.ps1"

if (-not (Test-Path -LiteralPath $checkScript)) {
  throw "VoidBot operations probe is missing at $checkScript."
}

Push-Location -LiteralPath $voidBotRoot
try {
  & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $checkScript -FailOnIssues
  exit $LASTEXITCODE
} finally {
  Pop-Location
}

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$bifrostRoot = "E:\Projects\Bifrost"
$publisherScript = Join-Path $bifrostRoot "tools\operator-notification.mjs"

if (-not (Test-Path -LiteralPath $publisherScript)) {
  throw "Bifrost operator-notification publisher is missing at $publisherScript."
}

Push-Location -LiteralPath $bifrostRoot
try {
  $arguments = @($publisherScript, "publish-idunn-alarm")
  if ($env:IDUNN_OPERATOR_ALARM_DRY_RUN -in @("1", "true", "yes", "on")) {
    $arguments += "--dry-run"
  }
  & node @arguments
  if ($LASTEXITCODE -ne 0) {
    throw "Bifrost operator-notification publisher exited with code $LASTEXITCODE."
  }
} finally {
  Pop-Location
}

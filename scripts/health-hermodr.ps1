param(
  [string] $BaseUrl = $(if ($env:HERMODR_BASE_URL) { $env:HERMODR_BASE_URL } else { "http://127.0.0.1:8798" })
)

$ErrorActionPreference = "Stop"

$healthUrl = $BaseUrl.TrimEnd("/") + "/health"
$response = Invoke-RestMethod -Uri $healthUrl -Method Get -TimeoutSec 3

if ($response.authority -ne "lowering-only") {
  throw "Hermodr responded, but did not report lowering-only authority."
}

Write-Host "Hermodr browser lowering responds at $BaseUrl; authority=$($response.authority). This is not daemon health or Verse discovery authority."

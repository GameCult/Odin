param(
  [string] $RavenHost = "10.77.0.4",
  [string] $HealthUrl = "http://127.0.0.1:8801/health"
)

$ErrorActionPreference = "Stop"

& ssh.exe -o BatchMode=yes -o ConnectTimeout=5 $RavenHost "curl.exe -fsS $HealthUrl >NUL"
exit $LASTEXITCODE
